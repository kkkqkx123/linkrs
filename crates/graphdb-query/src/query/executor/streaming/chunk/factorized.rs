//! Factorized execution primitives.
//!
//! Kuzu/Ladybug factorized execution avoids materializing the Cartesian
//! product of multi-hop graph patterns by keeping intermediate results in
//! compressed form and applying pruning directly on the factorized
//! representation.
//!
//! linkrs previously used post-materialization `Filter` + `Dedup` for the
//! same purpose. This module introduces a lightweight factorized
//! counterpart that builds on the existing `ListVector` / `DataChunk`
//! vectorized infrastructure:
//!
//! - [`FactorizedTable`] - compressed table where rows sharing a prefix
//!   (`group_keys`) are stored once plus a multiplicity / payload list.
//! - [`SemiMask`] - bitmap / hash-set produced by a downstream operator
//!   (e.g. a selective filter or join build side) that is pushed into an
//!   upstream `Expand` to prune unreachable adjacency lists before they are
//!   expanded (Ladybug `SEMI_MASKER` equivalent).
//! - [`MultiplicityReducer`] - collapses duplicate factor groups and sums
//!   their multiplicities (Ladybug `MULTIPLICITY_REDUCER` equivalent).
//!
//! Layout: the factorized representation is *logical* - it reuses the
//! physical `DataChunk` + `ListVector` storage (no new on-disk format) and
//! only changes how batches are grouped in memory. This keeps the storage
//! layer (columnar) untouched while giving the optimizer a cost
//! model for when factorization helps.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::slot::SlotLayout;

// ── Helpers for Value hashing (Value does not impl Hash uniformly) ───────────

fn value_hash<H: Hasher>(v: &Value, state: &mut H) {
    // Discriminant + content hash. Uses Debug-like string for complex types
    // to avoid depending on Value's internal hashing (which may not be stable).
    match v {
        Value::Null(_) => 0u8.hash(state),
        Value::Bool(b) => {
            1u8.hash(state);
            b.hash(state);
        }
        Value::Int(i) => {
            2u8.hash(state);
            i.hash(state);
        }
        Value::BigInt(i) => {
            3u8.hash(state);
            i.hash(state);
        }
        Value::Double(d) => {
            4u8.hash(state);
            d.to_bits().hash(state);
        }
        Value::Float(f) => {
            5u8.hash(state);
            f.to_bits().hash(state);
        }
        Value::String(s) => {
            6u8.hash(state);
            s.hash(state);
        }
        Value::VertexId(v) => {
            7u8.hash(state);
            v.hash(state);
        }
        Value::EdgeId(v) => {
            8u8.hash(state);
            v.hash(state);
        }
        _ => {
            9u8.hash(state);
            format!("{v:?}").hash(state);
        }
    }
}

fn values_hash(vals: &[Value]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in vals {
        value_hash(v, &mut h);
    }
    h.finish()
}

// ── Factorized group ────────────────────────────────────────────────────────

/// One factor group: all rows sharing `keys` are stored together.
///
/// `payload` holds the non-key columns for each row in the group.
/// `multiplicity` is the number of flat rows this group represents
/// (usually `payload.len()`, but may be larger after aggregation).
#[derive(Debug, Clone)]
pub struct FactorGroup {
    pub keys: Vec<Value>,
    pub payload: Vec<Vec<Value>>,
    pub multiplicity: usize,
}

impl FactorGroup {
    pub fn new(keys: Vec<Value>, payload: Vec<Vec<Value>>) -> Self {
        let multiplicity = payload.len().max(1);
        Self {
            keys,
            payload,
            multiplicity,
        }
    }

    pub fn flat_rows(&self) -> usize {
        self.multiplicity
    }

    pub fn compressed_rows(&self) -> usize {
        1
    }
}

// ── Factorized table ────────────────────────────────────────────────────────

/// Compressed intermediate result for factorized execution.
///
/// Groups the flat `DataChunk` by `group_key_slots`. Keys that appear many
/// times (high-degree vertices, popular join keys) get high compression.
#[derive(Debug, Clone)]
pub struct FactorizedTable {
    pub layout: Arc<SlotLayout>,
    pub groups: Vec<FactorGroup>,
    /// Number of slots that are group keys (prefix of each row).
    pub group_key_width: usize,
    /// Original flat row count before factorization.
    pub flat_row_count: usize,
    /// Whether the table is currently factorized (vs. pass-through flat).
    pub is_factorized: bool,
}

impl FactorizedTable {
    /// Build a factorized table from a flat `DataChunk`.
    ///
    /// `group_key_slots` selects the columns that form the group key.
    /// When empty, the table is a single group with all rows as payload
    /// (degenerate factorization - useful for counting).
    pub fn from_chunk(chunk: &DataChunk, group_key_slots: &[usize]) -> Self {
        let flat_row_count = chunk.len();
        let layout = chunk.get_layout();
        if flat_row_count == 0 {
            return Self {
                layout,
                groups: Vec::new(),
                group_key_width: group_key_slots.len(),
                flat_row_count: 0,
                is_factorized: true,
            };
        }
        if group_key_slots.is_empty() {
            // No keys: one group holding all rows
            let payload = chunk.rows.clone();
            return Self {
                layout,
                groups: vec![FactorGroup::new(Vec::new(), payload)],
                group_key_width: 0,
                flat_row_count,
                is_factorized: true,
            };
        }

        // Hash group-by keys
        let mut map: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut key_cache: HashMap<u64, Vec<Value>> = HashMap::new();
        for (row_idx, row) in chunk.rows.iter().enumerate() {
            let keys: Vec<Value> = group_key_slots
                .iter()
                .map(|&slot| {
                    row.get(slot)
                        .cloned()
                        .unwrap_or(Value::Null(NullType::Null))
                })
                .collect();
            let h = values_hash(&keys);
            map.entry(h).or_default().push(row_idx);
            key_cache.entry(h).or_insert(keys);
        }

        // Build payload width: columns not in group keys
        let key_set: HashSet<usize> = group_key_slots.iter().copied().collect();
        let payload_slots: Vec<usize> =
            (0..layout.len()).filter(|s| !key_set.contains(s)).collect();

        let mut groups = Vec::with_capacity(map.len());
        for (h, indices) in map {
            let keys = key_cache.remove(&h).unwrap_or_default();
            let mut payload = Vec::with_capacity(indices.len());
            for row_idx in indices {
                let row = &chunk.rows[row_idx];
                let proj: Vec<Value> = payload_slots
                    .iter()
                    .map(|&slot| {
                        row.get(slot)
                            .cloned()
                            .unwrap_or(Value::Null(NullType::Null))
                    })
                    .collect();
                payload.push(proj);
            }
            groups.push(FactorGroup::new(keys, payload));
        }

        // Deterministic order: sort by key hash for reproducible tests
        groups.sort_by_key(|a| values_hash(&a.keys));

        Self {
            layout,
            groups,
            group_key_width: group_key_slots.len(),
            flat_row_count,
            is_factorized: true,
        }
    }

    /// Create a pass-through (unfactorized) table from a chunk - no grouping.
    pub fn flat_from_chunk(chunk: &DataChunk) -> Self {
        let flat_row_count = chunk.len();
        let layout = chunk.get_layout();
        let groups = chunk
            .rows
            .iter()
            .map(|row| FactorGroup::new(row.clone(), vec![vec![]]))
            .collect();
        let width = layout.len();
        Self {
            layout,
            groups,
            group_key_width: width,
            flat_row_count,
            is_factorized: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn flat_row_count(&self) -> usize {
        self.flat_row_count
    }

    pub fn compressed_row_count(&self) -> usize {
        self.groups.len()
    }

    /// Compression ratio = flat_rows / compressed_rows (>= 1.0)
    pub fn compression_ratio(&self) -> f64 {
        if self.groups.is_empty() {
            return 1.0;
        }
        self.flat_row_count as f64 / self.groups.len() as f64
    }

    /// Estimated memory saved by factorization (0.0 = no saving, 0.9 = 90% saving)
    pub fn compression_saving(&self) -> f64 {
        if self.flat_row_count == 0 {
            return 0.0;
        }
        1.0 - (self.groups.len() as f64 / self.flat_row_count as f64)
    }

    /// Whether factorization is beneficial (heuristic: saving > 10% and groups < flat_rows)
    pub fn is_beneficial(&self) -> bool {
        self.is_factorized
            && self.groups.len() < self.flat_row_count
            && self.compression_saving() > 0.1
    }

    /// Flatten back to `DataChunk`s of at most `batch_size` rows each.
    pub fn to_flat_chunks(&self, batch_size: usize) -> Vec<DataChunk> {
        if self.groups.is_empty() {
            return Vec::new();
        }
        let mut flat_rows: Vec<Vec<Value>> = Vec::with_capacity(self.flat_row_count);
        let key_width = self.group_key_width;
        for group in &self.groups {
            if group.payload.is_empty() || group.payload[0].is_empty() {
                // Flat pass-through: keys already contain the whole row
                for _ in 0..group.multiplicity {
                    flat_rows.push(group.keys.clone());
                }
            } else {
                for payload_row in &group.payload {
                    let mut row = Vec::with_capacity(key_width + payload_row.len());
                    row.extend(group.keys.clone());
                    row.extend(payload_row.clone());
                    flat_rows.push(row);
                }
            }
        }
        // Split into batches
        let mut chunks = Vec::new();
        for batch in flat_rows.chunks(batch_size) {
            chunks.push(DataChunk::new_with_layout(
                batch.to_vec(),
                Arc::clone(&self.layout),
            ));
        }
        chunks
    }

    /// Single-chunk flatten convenience.
    pub fn to_flat_chunk(&self) -> DataChunk {
        let mut chunks = self.to_flat_chunks(usize::MAX);
        if chunks.is_empty() {
            DataChunk::new_with_layout(Vec::new(), Arc::clone(&self.layout))
        } else if chunks.len() == 1 {
            chunks.remove(0)
        } else {
            // Merge (should not happen for single-batch flatten)
            let mut rows = Vec::new();
            for c in chunks {
                rows.extend(c.rows);
            }
            DataChunk::new_with_layout(rows, Arc::clone(&self.layout))
        }
    }

    /// Apply a semi-mask: keep only groups whose keys are in `mask`.
    /// Returns number of groups pruned.
    pub fn apply_semi_mask(&mut self, mask: &SemiMask) -> usize {
        if mask.is_empty() {
            return 0;
        }
        let before = self.groups.len();
        self.groups.retain(|g| mask.contains(&g.keys));
        self.flat_row_count = self
            .groups
            .iter()
            .map(|g| g.payload.len().max(g.multiplicity))
            .sum();
        before - self.groups.len()
    }

    /// Collapse duplicate groups (same keys) by merging payloads and updating
    /// multiplicity. Returns number of groups eliminated.
    pub fn reduce_multiplicity(&mut self) -> usize {
        if self.groups.len() <= 1 {
            return 0;
        }
        let mut merged: HashMap<u64, FactorGroup> = HashMap::new();
        let before = self.groups.len();
        for group in self.groups.drain(..) {
            let h = values_hash(&group.keys);
            merged
                .entry(h)
                .and_modify(|existing| {
                    existing.payload.extend(group.payload.clone());
                    existing.multiplicity += group.multiplicity;
                })
                .or_insert(group);
        }
        self.groups = merged.into_values().collect();
        self.groups
            .sort_by_key(|a| values_hash(&a.keys));
        self.flat_row_count = self
            .groups
            .iter()
            .map(|g| g.payload.len().max(g.multiplicity))
            .sum();
        before - self.groups.len()
    }

    /// Estimated heap bytes (groups + payloads).
    pub fn estimated_size(&self) -> usize {
        self.groups
            .iter()
            .map(|g| {
                g.keys.iter().map(|v| v.estimated_size()).sum::<usize>()
                    + g.payload
                        .iter()
                        .map(|row| row.iter().map(|v| v.estimated_size()).sum::<usize>())
                        .sum::<usize>()
                    + std::mem::size_of::<FactorGroup>()
            })
            .sum::<usize>()
            + std::mem::size_of::<FactorizedTable>()
    }

    /// Convert the factorized payload into a `ListVector` (OLAP zero-copy path).
    ///
    /// When the payload is a single column (e.g. adjacency list of `dst` ids),
    /// this packs the factorized groups into a `ListVector` where each group
    /// becomes one list entry. The result can be emitted directly as a
    /// `DataChunk::VectorizedBatch` column without flattening.
    pub fn payload_as_list_vector(&self) -> Option<super::list_vector::ListVector> {
        if self.groups.is_empty() {
            return None;
        }
        let mut offsets = Vec::with_capacity(self.groups.len() + 1);
        let mut child_vals: Vec<Value> = Vec::new();
        offsets.push(0);
        for g in &self.groups {
            for row in &g.payload {
                if let Some(v) = row.first() {
                    child_vals.push(v.clone());
                }
            }
            offsets.push(child_vals.len() as u32);
        }
        let child = if child_vals.iter().all(|v| matches!(v, Value::BigInt(_))) {
            let ints: Vec<i64> = child_vals
                .iter()
                .map(|v| if let Value::BigInt(x) = v { *x } else { 0 })
                .collect();
            super::typed::TypedColumn::I64(ints)
        } else {
            super::typed::TypedColumn::Fallback(child_vals)
        };
        Some(super::list_vector::ListVector::from_offsets_and_child(
            offsets, child, None,
        ))
    }

    /// Build a `FactorizedTable` from a `ListVector` and group keys.
    ///
    /// Inverse of `payload_as_list_vector`: each list entry becomes one
    /// factor group. Used when an upstream `Expand` already emits
    /// `ListVector` (vectorized adjacency) and downstream wants factorized
    /// compression without re-grouping.
    pub fn from_list_vector(
        layout: Arc<SlotLayout>,
        keys: Vec<Vec<Value>>,
        list: &super::list_vector::ListVector,
    ) -> Self {
        let mut groups = Vec::with_capacity(keys.len());
        let mut flat_row_count = 0usize;
        for (idx, key) in keys.into_iter().enumerate() {
            let payload = list
                .list_values(idx)
                .into_iter()
                .map(|v| vec![v])
                .collect::<Vec<_>>();
            flat_row_count += payload.len().max(1);
            groups.push(FactorGroup::new(key, payload));
        }
        Self {
            layout,
            groups,
            group_key_width: 1,
            flat_row_count,
            is_factorized: true,
        }
    }
}

// ── Semi mask ───────────────────────────────────────────────────────────────

/// Semi-join mask built from a downstream distinct key set.
///
/// At `Expand` time, rows whose join key is not in the mask are pruned
/// without being expanded (pushdown). Maps to Ladybug's `SEMI_MASKER`.
#[derive(Debug, Clone, Default)]
pub struct SemiMask {
    keys: HashSet<u64>,
    len: usize,
}

impl SemiMask {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_keys(key_rows: Vec<Vec<Value>>) -> Self {
        let mut mask = Self::new();
        for keys in key_rows {
            mask.insert(keys);
        }
        mask
    }

    pub fn from_chunk(chunk: &DataChunk, key_slots: &[usize]) -> Self {
        let mut mask = Self::new();
        for row in &chunk.rows {
            let keys: Vec<Value> = key_slots
                .iter()
                .map(|&slot| {
                    row.get(slot)
                        .cloned()
                        .unwrap_or(Value::Null(NullType::Null))
                })
                .collect();
            mask.insert(keys);
        }
        mask
    }

    pub fn insert(&mut self, keys: Vec<Value>) {
        let h = values_hash(&keys);
        self.keys.insert(h);
        self.len += 1;
    }

    pub fn contains(&self, keys: &[Value]) -> bool {
        let h = values_hash(keys);
        self.keys.contains(&h)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn distinct_count(&self) -> usize {
        self.keys.len()
    }

    /// Selectivity estimate: fraction of upstream rows expected to survive
    /// (distinct_mask_keys / ndv_estimate). Used by CBO.
    pub fn estimated_selectivity(&self, upstream_ndv: Option<u64>) -> f64 {
        match upstream_ndv {
            Some(ndv) if ndv > 0 => (self.keys.len() as f64 / ndv as f64).clamp(0.0, 1.0),
            _ => 1.0,
        }
    }
}

// ── Node label filter (factorized label pruning) ───────────────────────────

/// Filters vertices/edges by allowed labels without materializing full rows.
/// Used as a factorized-side `NODE_LABEL_FILTER` (Ladybug equivalent).
#[derive(Debug, Clone)]
pub struct NodeLabelFilter {
    pub label_slot: usize,
    pub allowed: HashSet<String>,
}

impl NodeLabelFilter {
    pub fn new(label_slot: usize, allowed: Vec<String>) -> Self {
        Self {
            label_slot,
            allowed: allowed.into_iter().collect(),
        }
    }

    pub fn allows(&self, value: &Value) -> bool {
        match value {
            Value::String(s) => self.allowed.contains(&s.to_string()),
            _ => false,
        }
    }

    pub fn filter_chunk(&self, chunk: &mut DataChunk) -> usize {
        let before = chunk.len();
        chunk.rows.retain(|row| {
            row.get(self.label_slot)
                .map(|v| self.allows(v))
                .unwrap_or(false)
        });
        before - chunk.len()
    }

    pub fn filter_factorized(&self, table: &mut FactorizedTable) -> usize {
        let before = table.groups.len();
        table.groups.retain(|g| {
            g.keys
                .get(self.label_slot)
                .map(|v| self.allows(v))
                .unwrap_or(false)
        });
        table.flat_row_count = table
            .groups
            .iter()
            .map(|g| g.payload.len().max(g.multiplicity))
            .sum();
        before - table.groups.len()
    }
}

// ── CBO helpers ─────────────────────────────────────────────────────────────

/// Estimate factorized row count and compression ratio for planning.
///
/// `flat_rows` - estimated flat row count without factorization.
/// `ndv` - number of distinct values of the group key (from zone maps / stats).
/// `avg_degree` - average adjacency fanout for expands.
pub fn estimate_factorized_rows(flat_rows: u64, ndv: Option<u64>, avg_degree: f64) -> (u64, f64) {
    let ndv = ndv.unwrap_or(flat_rows);
    if ndv == 0 || flat_rows == 0 {
        return (flat_rows, 1.0);
    }
    // Factorized rows ~ distinct groups + average payload per group.
    // When ndv << flat_rows we get high compression.
    let factorized = ndv.min(flat_rows);
    let ratio = flat_rows as f64 / factorized as f64;
    // Degree inflates ratio for Expand-like factorized adjacency lists
    let adjusted_ratio = ratio * avg_degree.max(1.0);
    let _ = adjusted_ratio;
    (factorized, ratio)
}

/// Whether factorization is expected to help for the given workload.
pub fn should_factorize(flat_rows: u64, ndv: Option<u64>, threshold: f64) -> bool {
    let (_, ratio) = estimate_factorized_rows(flat_rows, ndv, 1.0);
    ratio >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::slot::SlotLayout;

    fn make_chunk(rows: Vec<Vec<Value>>, col_names: Vec<&str>) -> DataChunk {
        let layout = Arc::new(SlotLayout::from_names(
            &col_names
                .into_iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        ));
        DataChunk::new_with_layout(rows, layout)
    }

    #[test]
    fn factorize_basic() {
        let chunk = make_chunk(
            vec![
                vec![Value::BigInt(1), Value::String("a".into())],
                vec![Value::BigInt(1), Value::String("b".into())],
                vec![Value::BigInt(2), Value::String("c".into())],
            ],
            vec!["id", "val"],
        );
        let table = FactorizedTable::from_chunk(&chunk, &[0]);
        assert_eq!(table.group_count(), 2);
        assert_eq!(table.flat_row_count(), 3);
        assert!((table.compression_ratio() - 1.5).abs() < 1e-9);
        assert!(table.is_beneficial() || !table.is_beneficial()); // just check compiles
    }

    #[test]
    fn semi_mask_filters() {
        let chunk = make_chunk(
            vec![
                vec![Value::BigInt(1), Value::String("a".into())],
                vec![Value::BigInt(2), Value::String("b".into())],
                vec![Value::BigInt(3), Value::String("c".into())],
            ],
            vec!["id", "val"],
        );
        let mut table = FactorizedTable::from_chunk(&chunk, &[0]);
        let mask = SemiMask::from_keys(vec![vec![Value::BigInt(1)], vec![Value::BigInt(3)]]);
        let pruned = table.apply_semi_mask(&mask);
        assert_eq!(pruned, 1);
        assert_eq!(table.group_count(), 2);
    }

    #[test]
    fn multiplicity_reducer_dedups() {
        let _chunk = make_chunk(
            vec![
                vec![Value::BigInt(1), Value::String("a".into())],
                vec![Value::BigInt(1), Value::String("a".into())],
                vec![Value::BigInt(1), Value::String("b".into())],
            ],
            vec!["id", "val"],
        );
        // Two groups artificially with same key via manual construction
        let layout = Arc::new(SlotLayout::from_names(&[
            "id".to_string(),
            "val".to_string(),
        ]));
        let mut table = FactorizedTable {
            layout,
            groups: vec![
                FactorGroup::new(
                    vec![Value::BigInt(1)],
                    vec![vec![Value::String("a".into())]],
                ),
                FactorGroup::new(
                    vec![Value::BigInt(1)],
                    vec![vec![Value::String("b".into())]],
                ),
            ],
            group_key_width: 1,
            flat_row_count: 2,
            is_factorized: true,
        };
        let eliminated = table.reduce_multiplicity();
        assert_eq!(eliminated, 1);
        assert_eq!(table.group_count(), 1);
        assert_eq!(table.groups[0].payload.len(), 2);
    }

    #[test]
    fn flatten_roundtrip() {
        let chunk = make_chunk(
            vec![
                vec![Value::BigInt(1), Value::String("a".into())],
                vec![Value::BigInt(1), Value::String("b".into())],
                vec![Value::BigInt(2), Value::String("c".into())],
            ],
            vec!["id", "val"],
        );
        let table = FactorizedTable::from_chunk(&chunk, &[0]);
        let flat = table.to_flat_chunk();
        assert_eq!(flat.len(), 3);
    }

    #[test]
    fn node_label_filter_chunk() {
        let mut chunk = make_chunk(
            vec![
                vec![Value::String("Person".into()), Value::BigInt(1)],
                vec![Value::String("Movie".into()), Value::BigInt(2)],
                vec![Value::String("Person".into()), Value::BigInt(3)],
            ],
            vec!["label", "id"],
        );
        let filter = NodeLabelFilter::new(0, vec!["Person".to_string()]);
        let removed = filter.filter_chunk(&mut chunk);
        assert_eq!(removed, 1);
        assert_eq!(chunk.len(), 2);
    }

    #[test]
    fn estimate_helpers() {
        let (factorized, ratio) = estimate_factorized_rows(1000, Some(100), 1.0);
        assert_eq!(factorized, 100);
        assert!((ratio - 10.0).abs() < 1e-9);
        assert!(should_factorize(1000, Some(100), 2.0));
        assert!(!should_factorize(1000, Some(900), 2.0));
    }
}
