//! Bundled CSR: pure topology plus one inline scalar value column.
//!
//! Extends `PureTopologyCsr` with a `value: u64` column (20 bytes/edge)
//! for single numeric scalar attributes.  Type encoding/decoding is
//! centralized in `encode_scalar`/`decode_scalar`; the row stores only the
//! raw 64-bit representation.  The type is resolved from the published
//! schema at read time, never stored inline.
//!
//! The value column is slot-parallel to the topology: `primary_values` and
//! `primary_valid` run alongside the primary `endpoints`/`edge_ids` block,
//! and each topology overflow chunk has a parallel `BundledOverflowValues`
//! chunk of identical length.  Every topology mutation goes through the
//! shared `PureTopologyCsr` entry (`insert_edge_returning_position`,
//! positioned deletes) or mirrors its slot moves exactly (`rollback_insert`,
//! `compact_vertex_with_reporting`), so the two columns never drift.
//!
//! A deleted slot keeps its stale raw word but clears its validity bit;
//! the positional revert with value restores both.  The value-blind trait
//! revert restores the topology only and leaves the slot NULL.
//!
//! Known limitation: the frozen packer stores topology only, so freezing a
//! group with valid values is rejected; migrate to the columnar form first
//! when a freeze is required.
//!
//! Suitability boundary: this form fits exactly one inline scalar attribute
//! on a read-heavy, schema-stable edge type. Anything else (multiple
//! attributes, non-encodable types, online schema changes, freeze without a
//! prior migration) belongs to the columnar form, which is also the default.
//! The bundled form is intentionally not extended beyond a single column.

use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};

use super::csr_shared::SegmentedTable;
use super::csr_trait::{CsrBase, MutableCsrTrait};
use super::pure_csr::{PureOverflowChunk, PureTopologyCsr, DEFAULT_OVERFLOW_CHUNK_EDGES};
use super::{EdgePosition, Nbr};
use crate::persistence::{read_u32_le, read_u64_le};

const INVALID_EDGE_ID: EdgeId = EdgeId(u64::MAX);

/// Encode a scalar `Value` into a 64-bit storage word.
///
/// `Bool` uses 0/1, `Float` transmutes via `f32::to_bits`, and all integer
/// and time types are widened to `i64`.  Types not representable in 64 bits
/// are rejected at table creation; this function assumes the caller already
/// validated the type.
pub fn encode_scalar(v: &Value) -> u64 {
    match v {
        Value::Bool(b) => u64::from(*b),
        Value::SmallInt(n) => *n as u64,
        Value::Int(n) => *n as u64,
        Value::BigInt(n) => *n as u64,
        Value::Float(f) => f.to_bits() as u64,
        Value::Double(f) => f.to_bits(),
        Value::Date(d) => d.to_days() as u64,
        Value::Time(t) => {
            (t.hour as i64 * 3_600_000_000
                + t.minute as i64 * 60_000_000
                + t.sec as i64 * 1_000_000
                + t.microsec as i64) as u64
        }
        Value::DateTime(dt) => dt.to_micros() as u64,
        _ => 0,
    }
}

/// Decode a 64-bit storage word back into a `Value` of the given `DataType`.
pub fn decode_scalar(raw: u64, dt: &graphdb_core::DataType) -> Value {
    use graphdb_core::DataType;
    match dt {
        DataType::Bool => Value::Bool(raw != 0),
        DataType::SmallInt => Value::SmallInt(raw as i16),
        DataType::Int => Value::Int(raw as i32),
        DataType::BigInt => Value::BigInt(raw as i64),
        DataType::Float => Value::Float(f32::from_bits(raw as u32)),
        DataType::Double => Value::Double(f64::from_bits(raw)),
        DataType::Date => Value::Date(graphdb_core::value::DateValue::from_days(raw as i64)),
        DataType::Time => {
            let total_micros = raw as i64;
            let microsec = (total_micros % 1_000_000) as u32;
            let total_secs = total_micros / 1_000_000;
            let sec = (total_secs % 60) as u32;
            let total_mins = total_secs / 60;
            let minute = (total_mins % 60) as u32;
            let hour = (total_mins / 60) as u32;
            Value::Time(graphdb_core::value::TimeValue {
                hour,
                minute,
                sec,
                microsec,
            })
        }
        DataType::DateTime => {
            Value::DateTime(graphdb_core::value::DateTimeValue::from_micros(raw as i64))
        }
        _ => Value::Empty,
    }
}

/// Overflow value chunk parallel to one topology overflow chunk.
///
/// Lengths are kept identical to the paired topology chunk; every topology
/// push/remove on the row applies the same operation here.
#[derive(Debug, Clone, Default)]
struct BundledOverflowValues {
    values: Vec<u64>,
    valid: Vec<bool>,
}

impl BundledOverflowValues {
    fn with_capacity(cap: usize) -> Self {
        Self {
            values: Vec::with_capacity(cap),
            valid: Vec::with_capacity(cap),
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.values.len()
    }

    #[inline]
    fn remove(&mut self, index: usize) {
        self.values.remove(index);
        self.valid.remove(index);
    }

    fn consolidated(values: &[u64], valid: &[bool]) -> Self {
        Self {
            values: values.to_vec(),
            valid: valid.to_vec(),
        }
    }

    fn heap_bytes(&self) -> usize {
        self.values.len() * 8 + self.valid.len()
    }
}

/// Bundled CSR: pure topology + one inline `value: u64` column.
///
/// The value column never participates in dedup or routing; it strictly
/// follows the topology slots.
#[derive(Debug, Clone)]
pub struct BundledCsr {
    /// Underlying pure topology (endpoint + edge_id columns).
    topology: PureTopologyCsr,
    /// Per-slot value column, parallel to the topology primary block.
    primary_values: Vec<u64>,
    /// Per-slot validity bit, parallel to the topology primary block.
    /// True means the slot holds a valid value; false means NULL.
    primary_valid: Vec<bool>,
    /// Per-overflow-chunk value columns, keyed by vertex like the topology
    /// overflow table with identical chunk counts and lengths.
    overflow_values: SegmentedTable<Vec<BundledOverflowValues>>,
}

impl Default for BundledCsr {
    fn default() -> Self {
        Self::new()
    }
}

impl BundledCsr {
    pub fn new() -> Self {
        Self::with_capacity(1024, 4096)
    }

    pub fn with_capacity(vertex_capacity: usize, edge_capacity: usize) -> Self {
        Self::with_overflow_chunk_edges(
            vertex_capacity,
            edge_capacity,
            DEFAULT_OVERFLOW_CHUNK_EDGES,
        )
    }

    pub fn with_overflow_chunk_edges(
        vertex_capacity: usize,
        edge_capacity: usize,
        overflow_chunk_edges: usize,
    ) -> Self {
        Self {
            topology: PureTopologyCsr::with_overflow_chunk_edges(
                vertex_capacity,
                edge_capacity,
                overflow_chunk_edges,
            ),
            primary_values: Vec::with_capacity(edge_capacity),
            primary_valid: Vec::with_capacity(edge_capacity),
            overflow_values: SegmentedTable::new(),
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.topology.vertex_capacity()
    }

    pub fn edge_count(&self) -> u64 {
        self.topology.edge_count()
    }

    pub fn clear(&mut self) {
        self.topology.clear();
        self.primary_values.clear();
        self.primary_valid.clear();
        self.overflow_values.clear();
    }

    pub fn visit_physical<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        self.topology.visit_physical(src_vid, f)
    }

    /// Borrowed row walk over live entries without allocating.
    ///
    /// Shares the topology walk exactly: values stay in their columns and
    /// are resolved through the value accessors when needed.
    pub fn iter_row(&self, src_vid: u32) -> super::pure_csr::PureRowIter<'_> {
        self.topology.iter_row(src_vid)
    }

    /// Borrowed walk over every live entry of the table without allocating.
    pub fn iter_all(&self) -> super::pure_csr::PureAllIter<'_> {
        self.topology.iter_all()
    }

    pub fn visit_hot<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(super::HotNbr) -> bool,
    {
        self.topology.visit_hot(src_vid, f)
    }

    /// Whether the live endpoints of one row arrive in ascending order.
    pub fn is_row_sorted(&self, src_vid: u32) -> bool {
        self.topology.is_row_sorted(src_vid)
    }

    /// Sort one primary row into `(endpoint, edge_id)` order with values.
    ///
    /// Same maintenance-only invalidation as the pure form; the value
    /// columns permute with the topology slots so no column drifts.
    /// Overflow chunks stay in insertion order as the unsorted suffix.
    pub fn sort_row(&mut self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.topology.vertex_capacity() {
            return false;
        }
        self.sync_primary_len();
        let (start, end) = self.topology.primary_window(idx);
        if end.saturating_sub(start) <= 1 {
            return false;
        }
        let mut live: Vec<(u32, u64, u64, bool)> = Vec::new();
        for i in start..end {
            let eid = self.topology.edge_ids[i];
            if eid != INVALID_EDGE_ID.0 {
                live.push((
                    self.topology.endpoints[i],
                    eid,
                    self.primary_values[i],
                    self.primary_valid[i],
                ));
            }
        }
        if live.len() <= 1 {
            return false;
        }
        let mut sorted = live.clone();
        sorted.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        if sorted
            .iter()
            .map(|(e, id, _, _)| (*e, *id))
            .eq(live.iter().map(|(e, id, _, _)| (*e, *id)))
        {
            return false;
        }
        for (offset, (endpoint, eid, value, valid)) in sorted.iter().enumerate() {
            self.topology.endpoints[start + offset] = *endpoint;
            self.topology.edge_ids[start + offset] = *eid;
            self.primary_values[start + offset] = *value;
            self.primary_valid[start + offset] = *valid;
        }
        for i in start + sorted.len()..end {
            self.topology.endpoints[i] = u32::MAX;
            self.topology.edge_ids[i] = INVALID_EDGE_ID.0;
            self.primary_values[i] = 0;
            self.primary_valid[i] = false;
        }
        self.topology.rebuild_live_set_for_vertex(vid);
        true
    }

    /// Sort every primary row that is out of order. Returns reordered rows.
    pub fn sort_all_rows(&mut self) -> usize {
        let rows = self.topology.vertex_capacity();
        let mut reordered = 0usize;
        for vid in 0..rows {
            if self.sort_row(vid as u32) {
                reordered += 1;
            }
        }
        reordered
    }

    /// Visit live entries whose endpoint falls in the inclusive range.
    pub fn visit_threshold<F>(&self, src_vid: u32, lower: Option<u32>, upper: Option<u32>, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        self.topology.visit_threshold(src_vid, lower, upper, f)
    }

    /// Fill a caller buffer with the same range content as `visit_threshold`.
    pub fn fill_threshold_into(
        &self,
        src_vid: u32,
        lower: Option<u32>,
        upper: Option<u32>,
        out: &mut Vec<Nbr>,
    ) {
        self.topology
            .fill_threshold_into(src_vid, lower, upper, out)
    }

    /// Visit every physically stored entry of one vertex together with its
    /// inline value (`None` for NULL slots).
    pub fn visit_physical_with_values<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr, Option<u64>) -> bool,
    {
        self.topology
            .visit_physical_with_position(src_vid, |position, nbr| {
                f(
                    nbr,
                    self.value_at_position(src_vid, position)
                        .and_then(|(raw, valid)| if valid { Some(raw) } else { None }),
                )
            })
    }

    /// Whether any slot in the table holds a valid (non-NULL) value.
    pub fn any_valid_values(&self) -> bool {
        if self.primary_valid.iter().any(|v| *v) {
            return true;
        }
        self.overflow_values
            .iter()
            .any(|(_, chunks)| chunks.iter().any(|c| c.valid.iter().any(|v| *v)))
    }

    /// Grow the primary value columns to cover the topology primary block.
    fn sync_primary_len(&mut self) {
        let want = self.topology.endpoints.len();
        if self.primary_values.len() < want {
            self.primary_values.resize(want, 0);
        }
        if self.primary_valid.len() < want {
            self.primary_valid.resize(want, false);
        }
    }

    /// Grow the overflow value row to cover the topology overflow row.
    fn sync_overflow_row(&mut self, vid: u32) {
        let topo_lens: Vec<usize> = self
            .topology
            .overflow_chunks
            .get(vid)
            .map(|chunks| chunks.iter().map(|c| c.len()).collect())
            .unwrap_or_default();
        if topo_lens.is_empty() {
            if self.overflow_values.get(vid).is_some() {
                self.overflow_values.take(vid);
            }
            return;
        }
        self.overflow_values
            .ensure_capacity(self.topology.vertex_capacity());
        let slot = self.overflow_values.slot_mut(vid);
        if slot.is_none() {
            *slot = Some(Vec::new());
        }
        let chunks = slot.as_mut().expect("overflow value row just created");
        while chunks.len() < topo_lens.len() {
            let want = topo_lens[chunks.len()];
            let mut fresh = BundledOverflowValues::with_capacity(want.max(1));
            fresh.values.resize(want, 0);
            fresh.valid.resize(want, false);
            chunks.push(fresh);
        }
        for (chunk, want) in chunks.iter_mut().zip(topo_lens.iter()) {
            if chunk.len() < *want {
                chunk.values.resize(*want, 0);
                chunk.valid.resize(*want, false);
            }
        }
    }

    fn primary_index(&self, src_vid: u32, slot: u32) -> Option<usize> {
        let src_idx = src_vid as usize;
        if src_idx >= self.topology.vertex_capacity() {
            return None;
        }
        if slot as usize >= self.topology.rows.degrees[src_idx] as usize {
            return None;
        }
        let idx = self.topology.rows.adj_offsets[src_idx] as usize + slot as usize;
        if idx >= self.topology.edge_ids.len() || idx >= self.primary_values.len() {
            return None;
        }
        Some(idx)
    }

    /// Read the raw value and validity of one physical slot.
    pub fn value_at_position(&self, src_vid: u32, position: EdgePosition) -> Option<(u64, bool)> {
        match position {
            EdgePosition::Primary { slot } => {
                let idx = self.primary_index(src_vid, slot)?;
                Some((self.primary_values[idx], self.primary_valid[idx]))
            }
            EdgePosition::Overflow { chunk, slot } => {
                let chunks = self.overflow_values.get(src_vid)?;
                let c = chunks.get(chunk as usize)?;
                let value = *c.values.get(slot as usize)?;
                let valid = *c.valid.get(slot as usize)?;
                Some((value, valid))
            }
        }
    }

    /// Overwrite the value of one physical slot holding a live edge.
    pub fn set_value_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        value: Option<u64>,
    ) -> bool {
        match position {
            EdgePosition::Primary { slot } => {
                let Some(idx) = self.primary_index(src_vid, slot) else {
                    return false;
                };
                if self.topology.edge_ids[idx] == INVALID_EDGE_ID.0 {
                    return false;
                }
                let (raw, valid) = match value {
                    Some(v) => (v, true),
                    None => (0, false),
                };
                self.primary_values[idx] = raw;
                self.primary_valid[idx] = valid;
                true
            }
            EdgePosition::Overflow { chunk, slot } => {
                let topo_live = self
                    .topology
                    .overflow_chunks
                    .get(src_vid)
                    .and_then(|chunks| chunks.get(chunk as usize))
                    .and_then(|c| c.edge_ids.get(slot as usize))
                    .is_some_and(|eid| *eid != INVALID_EDGE_ID.0);
                if !topo_live {
                    return false;
                }
                let Some(chunks) = self.overflow_values.get_mut(src_vid) else {
                    return false;
                };
                let Some(c) = chunks.get_mut(chunk as usize) else {
                    return false;
                };
                if slot as usize >= c.len() {
                    return false;
                }
                let (raw, valid) = match value {
                    Some(v) => (v, true),
                    None => (0, false),
                };
                c.values[slot as usize] = raw;
                c.valid[slot as usize] = valid;
                true
            }
        }
    }

    /// Insert one edge carrying its inline value (`None` stores NULL).
    pub fn insert_edge_with_value(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        value: Option<u64>,
    ) -> StorageResult<()> {
        let position = self
            .topology
            .insert_edge_returning_position(src_vid, dst, edge_id)?;
        self.sync_primary_len();
        self.sync_overflow_row(src_vid);
        let ok = self.set_value_at_position(src_vid, position, value);
        if !ok {
            return Err(StorageError::data_corruption(format!(
                "bundled value slot missing after insert of edge {:?}",
                edge_id
            )));
        }
        Ok(())
    }

    /// Read the value of one edge by id within its source row.
    pub fn value_by_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(u64, bool)> {
        let (position, _) = self.topology.locate_edge(src_vid, edge_id)?;
        self.value_at_position(src_vid, position)
    }

    /// Overwrite the value of one edge by id within its source row.
    pub fn set_value_by_edge_id(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        value: Option<u64>,
    ) -> bool {
        let Some((position, _)) = self.topology.locate_edge(src_vid, edge_id) else {
            return false;
        };
        self.set_value_at_position(src_vid, position, value)
    }

    /// Read the value of the live edge for one endpoint.
    pub fn value_by_endpoint(&self, src_vid: u32, endpoint: u32) -> Option<(u64, bool)> {
        let mut found = None;
        self.topology
            .visit_physical_with_position(src_vid, |position, nbr| {
                if nbr.endpoint == endpoint && nbr.edge_id != INVALID_EDGE_ID {
                    found = self.value_at_position(src_vid, position);
                    false
                } else {
                    true
                }
            });
        found
    }

    /// Overwrite the value of the live edge for one endpoint.
    pub fn set_value_by_endpoint(
        &mut self,
        src_vid: u32,
        endpoint: u32,
        value: Option<u64>,
    ) -> bool {
        let mut target = None;
        self.topology
            .visit_physical_with_position(src_vid, |position, nbr| {
                if nbr.endpoint == endpoint && nbr.edge_id != INVALID_EDGE_ID {
                    target = Some(position);
                    false
                } else {
                    true
                }
            });
        match target {
            Some(position) => self.set_value_at_position(src_vid, position, value),
            None => false,
        }
    }

    /// Revert a deletion, restoring the caller's value alongside the topology.
    pub fn revert_delete_at_position_with_value(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
        value: Option<u64>,
    ) -> bool {
        if !self
            .topology
            .revert_delete_at_position(src_vid, position, expected, ts)
        {
            return false;
        }
        self.sync_primary_len();
        self.sync_overflow_row(src_vid);
        self.set_value_at_position(src_vid, position, value)
    }

    /// Clear the validity bit of one physical slot, keeping the stale word.
    fn clear_value_at_position(&mut self, src_vid: u32, position: EdgePosition) {
        match position {
            EdgePosition::Primary { slot } => {
                if let Some(idx) = self.primary_index(src_vid, slot) {
                    if idx < self.primary_valid.len() {
                        self.primary_valid[idx] = false;
                    }
                }
            }
            EdgePosition::Overflow { chunk, slot } => {
                if let Some(chunks) = self.overflow_values.get_mut(src_vid) {
                    if let Some(c) = chunks.get_mut(chunk as usize) {
                        if (slot as usize) < c.valid.len() {
                            c.valid[slot as usize] = false;
                        }
                    }
                }
            }
        }
    }

    /// Erase one just-inserted edge for insert rollback, shifting the value column with the topology.
    fn rollback_insert_shifted(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        if edge_id == INVALID_EDGE_ID {
            return false;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.topology.vertex_capacity() {
            return false;
        }
        let found = {
            let (start, end) = self.topology.primary_window(src_idx);
            self.topology.edge_ids[start..end]
                .iter()
                .enumerate()
                .find_map(|(i, &eid)| {
                    if EdgeId(eid) == edge_id {
                        Some(start + i)
                    } else {
                        None
                    }
                })
        };
        if let Some(idx) = found {
            let degree = self.topology.rows.degrees[src_idx] as usize;
            let base = self.topology.rows.adj_offsets[src_idx] as usize;
            self.topology
                .endpoints
                .copy_within(idx + 1..base + degree, idx);
            self.topology
                .edge_ids
                .copy_within(idx + 1..base + degree, idx);
            self.sync_primary_len();
            self.primary_values.copy_within(idx + 1..base + degree, idx);
            self.primary_valid.copy_within(idx + 1..base + degree, idx);
            self.topology.rows.degrees[src_idx] -= 1;
            self.topology.sub_capacity(1);
            self.topology.edge_count -= 1;
            self.topology.rebuild_live_set_for_vertex(src_vid);
            return true;
        }
        let located = self.topology.scan_overflow_for_edge_id(src_vid, edge_id);
        if let Some((chunk_idx, edge_idx)) = located {
            if let Some(chunks) = self.topology.overflow_chunks.get_mut(src_vid) {
                chunks[chunk_idx].remove(edge_idx);
                let emptied = chunks[chunk_idx].is_empty();
                let mut freed = None;
                if emptied {
                    let removed = chunks.remove(chunk_idx);
                    freed = Some((removed.capacity(), chunks.is_empty()));
                }
                if let Some(vchunks) = self.overflow_values.get_mut(src_vid) {
                    if let Some(vc) = vchunks.get_mut(chunk_idx) {
                        if edge_idx < vc.len() {
                            vc.remove(edge_idx);
                        }
                    }
                    if emptied && chunk_idx < vchunks.len() {
                        vchunks.remove(chunk_idx);
                    }
                    if vchunks.is_empty() {
                        self.overflow_values.take(src_vid);
                    }
                }
                self.topology.edge_count -= 1;
                if let Some((freed_cap, all_gone)) = freed {
                    self.topology.sub_capacity(freed_cap);
                    if all_gone {
                        self.topology.overflow_chunks.remove(src_vid);
                    }
                }
                self.topology.rebuild_live_set_for_vertex(src_vid);
                return true;
            }
        }
        false
    }

    /// Compact one vertex, moving the value column with the topology.
    fn compact_vertex_shifted(
        &mut self,
        vid: u32,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        let idx = vid as usize;
        if idx >= self.topology.vertex_capacity() {
            return 0;
        }
        let mut removed = 0usize;
        self.sync_primary_len();
        self.sync_overflow_row(vid);
        let base = self.topology.rows.adj_offsets[idx] as usize;
        let degree = self.topology.rows.degrees[idx] as usize;
        let mut keep = 0usize;
        for i in 0..degree {
            let slot = base + i;
            if self.topology.edge_ids[slot] == INVALID_EDGE_ID.0 {
                // Holes carry no edge identity: drop them (and their stale
                // words) silently instead of reporting the sentinel upward.
                removed += 1;
            } else {
                if keep != i {
                    self.topology.endpoints[base + keep] = self.topology.endpoints[slot];
                    self.topology.edge_ids[base + keep] = self.topology.edge_ids[slot];
                    self.primary_values[base + keep] = self.primary_values[slot];
                    self.primary_valid[base + keep] = self.primary_valid[slot];
                }
                keep += 1;
            }
        }
        self.topology.rows.degrees[idx] = keep as u32;
        self.topology.rows.primary_capacities[idx] = keep as u32;
        if self.topology.overflow_chunks.get(vid).is_some() {
            let chunks = self
                .topology
                .overflow_chunks
                .remove(vid)
                .unwrap_or_default();
            let value_chunks = self.overflow_values.take(vid).unwrap_or_default();
            let freed: usize = chunks.iter().map(|chunk| chunk.capacity()).sum();
            self.topology.sub_capacity(freed);
            let mut kept_ep: Vec<u32> = Vec::new();
            let mut kept_eid: Vec<u64> = Vec::new();
            let mut kept_val: Vec<u64> = Vec::new();
            let mut kept_ok: Vec<bool> = Vec::new();
            for (chunk_no, chunk) in chunks.iter().enumerate() {
                let values = value_chunks.get(chunk_no);
                for i in 0..chunk.len() {
                    let eid = chunk.edge_ids[i];
                    if eid == INVALID_EDGE_ID.0 {
                        removed += 1;
                    } else {
                        kept_ep.push(chunk.endpoints[i]);
                        kept_eid.push(eid);
                        kept_val.push(values.and_then(|v| v.values.get(i)).copied().unwrap_or(0));
                        kept_ok.push(
                            values
                                .and_then(|v| v.valid.get(i))
                                .copied()
                                .unwrap_or(false),
                        );
                    }
                }
            }
            if !kept_ep.is_empty() {
                let single = PureOverflowChunk::consolidated(&kept_ep, &kept_eid);
                let added = single.capacity();
                self.topology.add_capacity(added);
                self.topology.overflow_chunks.insert(vid, vec![single]);
                self.overflow_values
                    .ensure_capacity(self.topology.vertex_capacity());
                let slot = self.overflow_values.slot_mut(vid);
                *slot = Some(vec![BundledOverflowValues::consolidated(
                    &kept_val, &kept_ok,
                )]);
            }
            self.topology.rebuild_live_set_for_vertex(vid);
        } else if removed > 0 {
            self.topology.rebuild_live_set_for_vertex(vid);
        }
        removed
    }

    fn dump_valid_bits(out: &mut Vec<u8>, valid: &[bool]) {
        out.extend_from_slice(&(valid.len() as u32).to_le_bytes());
        for chunk in valid.chunks(8) {
            let mut byte = 0u8;
            for (i, v) in chunk.iter().enumerate() {
                if *v {
                    byte |= 1u8 << i;
                }
            }
            out.push(byte);
        }
    }

    fn load_valid_bits(data: &[u8], offset: &mut usize) -> StorageResult<Vec<bool>> {
        let count = read_u32_le(data, offset)? as usize;
        let need = count.div_ceil(8);
        if data.len().saturating_sub(*offset) < need {
            return Err(StorageError::deserialize_error(
                "bundled CSR valid-bit column too short",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let byte = data[*offset + i / 8];
            out.push(byte & (1u8 << (i % 8)) != 0);
        }
        *offset += need;
        // Padding bits must be zero, otherwise the payload is corrupt.
        if count % 8 != 0 {
            let last = data[*offset - 1];
            let used = count % 8;
            if last >> used != 0 {
                return Err(StorageError::deserialize_error(
                    "bundled CSR valid-bit padding corrupt",
                ));
            }
        }
        Ok(out)
    }

    fn dump_values_u64(out: &mut Vec<u8>, values: &[u64]) {
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for &v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn load_values_u64(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u64>> {
        let count = read_u32_le(data, offset)? as usize;
        let need = count.saturating_mul(8);
        if data.len().saturating_sub(*offset) < need {
            return Err(StorageError::deserialize_error(
                "bundled CSR value column too short",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&data[*offset..*offset + 8]);
            *offset += 8;
            out.push(u64::from_le_bytes(buf));
        }
        Ok(out)
    }
}

impl CsrBase for BundledCsr {
    fn vertex_capacity(&self) -> usize {
        self.topology.vertex_capacity()
    }

    fn edge_count(&self) -> u64 {
        self.topology.edge_count()
    }

    fn dump(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.dump_into(&mut out);
        out
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 12 {
            return Err(StorageError::deserialize_error(
                "bundled csr: data too short".to_string(),
            ));
        }
        let (body, trailer) = data.split_at(data.len() - 4);
        let mut stored_bytes = [0u8; 4];
        stored_bytes.copy_from_slice(trailer);
        let stored = u32::from_le_bytes(stored_bytes);
        let computed = crc32fast::hash(body);
        if stored != computed {
            return Err(StorageError::deserialize_error(format!(
                "bundled csr: dump CRC mismatch: stored={:#x} computed={:#x}",
                stored, computed
            )));
        }
        let data = body;
        let mut offset = 0usize;
        let topo_len = read_u64_le(data, &mut offset)? as usize;
        if data.len().saturating_sub(offset) < topo_len {
            return Err(StorageError::deserialize_error(
                "bundled csr: topology payload truncated",
            ));
        }
        self.topology.load(&data[offset..offset + topo_len])?;
        offset += topo_len;

        let primary_len = self.topology.endpoints.len();
        let values = Self::load_values_u64(data, &mut offset)?;
        let valid = Self::load_valid_bits(data, &mut offset)?;
        if values.len() != primary_len || valid.len() != primary_len {
            return Err(StorageError::deserialize_error(
                "bundled csr: primary value column length mismatch",
            ));
        }
        let vertex_capacity = self.topology.vertex_capacity();
        let mut overflow_values = SegmentedTable::new();
        overflow_values.ensure_capacity(vertex_capacity);
        for vid in 0..vertex_capacity {
            let chunk_count = read_u32_le(data, &mut offset)? as usize;
            let topo_count = self
                .topology
                .overflow_chunks
                .get(vid as u32)
                .map_or(0, Vec::len);
            if chunk_count != topo_count {
                return Err(StorageError::deserialize_error(
                    "bundled csr: overflow chunk count mismatch",
                ));
            }
            let mut chunks = Vec::with_capacity(chunk_count);
            for _ in 0..chunk_count {
                let chunk_len = read_u32_le(data, &mut offset)? as usize;
                let chunk_values = Self::load_values_u64(data, &mut offset)?;
                let chunk_valid = Self::load_valid_bits(data, &mut offset)?;
                if chunk_values.len() != chunk_len || chunk_valid.len() != chunk_len {
                    return Err(StorageError::deserialize_error(
                        "bundled csr: overflow value chunk length mismatch",
                    ));
                }
                chunks.push(BundledOverflowValues {
                    values: chunk_values,
                    valid: chunk_valid,
                });
            }
            // Cross-check lengths against the topology chunks.
            if let Some(topo) = self.topology.overflow_chunks.get(vid as u32) {
                for (a, b) in topo.iter().zip(chunks.iter()) {
                    if a.len() != b.len() {
                        return Err(StorageError::deserialize_error(
                            "bundled csr: overflow value length diverges from topology",
                        ));
                    }
                }
            }
            if !chunks.is_empty() {
                let slot = overflow_values.slot_mut(vid as u32);
                *slot = Some(chunks);
            }
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "bundled csr: trailing bytes after payload",
            ));
        }
        self.primary_values = values;
        self.primary_valid = valid;
        self.overflow_values = overflow_values;
        Ok(())
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        let start = out.len();
        let topo = self.topology.dump();
        out.extend_from_slice(&(topo.len() as u64).to_le_bytes());
        out.extend_from_slice(&topo);
        Self::dump_values_u64(out, &self.primary_values);
        Self::dump_valid_bits(out, &self.primary_valid);
        for vid in 0..self.topology.vertex_capacity() {
            match self.overflow_values.get(vid as u32) {
                Some(chunks) => {
                    out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
                    for chunk in chunks {
                        out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                        Self::dump_values_u64(out, &chunk.values);
                        Self::dump_valid_bits(out, &chunk.valid);
                    }
                }
                None => out.extend_from_slice(&0u32.to_le_bytes()),
            }
        }
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }
}

impl MutableCsrTrait for BundledCsr {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        self.insert_edge_with_value(src_vid, dst, edge_id, None)
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        let located = self.topology.locate_edge(src_vid, edge_id);
        let deleted = self.topology.delete_edge(src_vid, edge_id, ts)?;
        if deleted {
            if let Some((position, _)) = located {
                self.clear_value_at_position(src_vid, position);
            }
        }
        Ok(deleted)
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        let mut noop = |_: EdgeId| {};
        self.delete_edge_by_dst_reporting(src_vid, dst, ts, &mut noop)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        let mut positioned = |edge_id: EdgeId, position: Option<EdgePosition>| {
            on_deleted(edge_id);
            let _ = position;
        };
        self.delete_edge_by_dst_reporting_positioned(src_vid, dst, ts, &mut positioned)
    }

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        let mut stamped: Vec<(EdgeId, EdgePosition)> = Vec::new();
        let deleted = self.topology.delete_edge_by_dst_reporting_positioned(
            src_vid,
            dst,
            ts,
            &mut |edge_id, position| {
                if let Some(position) = position {
                    stamped.push((edge_id, position));
                }
            },
        );
        for (edge_id, position) in &stamped {
            self.clear_value_at_position(src_vid, *position);
            on_deleted(*edge_id, Some(*position));
        }
        // Topology positions are this variant's positions, so a complete
        // report means every stamped edge carried one.
        debug_assert_eq!(stamped.len(), deleted);
        deleted
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        self.topology.locate_edge(src_vid, edge_id)
    }

    fn delete_edge_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        let deleted = self
            .topology
            .delete_edge_at_position(src_vid, position, expected, ts)?;
        if deleted {
            self.clear_value_at_position(src_vid, position);
        }
        Ok(deleted)
    }

    fn revert_delete_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> bool {
        if !self
            .topology
            .revert_delete_at_position(src_vid, position, expected, ts)
        {
            return false;
        }
        // Holes are never reused by inserts, so the stale raw word still
        // belongs to this edge: revive its validity instead of NULLing it.
        // Callers carrying an explicit value use
        // `revert_delete_at_position_with_value`.
        self.sync_primary_len();
        self.sync_overflow_row(src_vid);
        match position {
            EdgePosition::Primary { slot } => {
                if let Some(idx) = self.primary_index(src_vid, slot) {
                    if idx < self.primary_valid.len() {
                        self.primary_valid[idx] = true;
                    }
                }
                true
            }
            EdgePosition::Overflow { chunk, slot } => {
                if let Some(chunks) = self.overflow_values.get_mut(src_vid) {
                    if let Some(c) = chunks.get_mut(chunk as usize) {
                        if (slot as usize) < c.valid.len() {
                            c.valid[slot as usize] = true;
                        }
                    }
                }
                true
            }
        }
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if offset < 0 {
            return Ok(false);
        }
        let position = EdgePosition::Primary {
            slot: offset as u32,
        };
        let deleted = self.topology.delete_edge_by_offset(src_vid, offset, ts)?;
        if deleted {
            self.clear_value_at_position(src_vid, position);
        }
        Ok(deleted)
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        // Topology slots erase the edge id on delete, so offset-only and
        // id-only reverts cannot recover it. Use the positioned revert with
        // the expected id and value supplied by the caller.
        let _ = (src_vid, offset, ts);
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        self.topology.nbr_at_offset(src_vid, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        self.topology.get_edge_physical(src_vid, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        self.topology.physical_edges_of(src_vid)
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        self.topology.fill_physical_into(src_vid, out)
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        self.topology.has_physical_entries(vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        self.topology.primary_contains(src_vid, edge_id)
    }

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        self.rollback_insert_shifted(src_vid, edge_id)
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        // Deleted topology slots hold the sentinel instead of the edge id,
        // so an id-keyed scan cannot locate them. Use the positioned revert
        // with value instead.
        let _ = (src_vid, edge_id, ts);
        false
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        self.topology.get_edge(src_vid, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        self.topology.edges_of(src_vid, ts)
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        _cutoff: Timestamp,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        self.compact_vertex_shifted(vid, _on_edge_removed)
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        self.topology.reclaimable_count(vid, cutoff)
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        self.topology.vertex_census(vid)
    }

    fn row_gap(&self, vid: u32) -> usize {
        self.topology.row_gap(vid)
    }

    fn row_density(&self, vid: u32) -> f32 {
        self.topology.row_density(vid)
    }

    fn rebalance_row(&mut self, vid: u32) -> bool {
        self.topology.rebalance_row(vid)
    }

    fn used_memory_size(&self) -> usize {
        self.topology.used_memory_size()
            + self.primary_values.len() * 8
            + self.primary_valid.len()
            + self
                .overflow_values
                .iter()
                .map(|(_, chunks)| {
                    chunks
                        .iter()
                        .map(BundledOverflowValues::heap_bytes)
                        .sum::<usize>()
                })
                .sum::<usize>()
            + self.overflow_values.table_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::VertexId;

    fn dst(endpoint: u32) -> VertexId {
        VertexId::edge_endpoint_key(endpoint, 0)
    }

    #[test]
    fn insert_and_point_read_roundtrip() {
        let mut csr = BundledCsr::with_overflow_chunk_edges(8, 0, 4);
        csr.insert_edge_with_value(0, dst(1), EdgeId(10), Some(0xDEAD_BEEF))
            .expect("insert with value");
        csr.insert_edge(0, dst(2), EdgeId(11), 0)
            .expect("insert without value stores NULL");
        assert_eq!(csr.edge_count(), 2);
        assert_eq!(
            csr.value_by_edge_id(0, EdgeId(10)),
            Some((0xDEAD_BEEF, true))
        );
        assert_eq!(csr.value_by_edge_id(0, EdgeId(11)), Some((0, false)));
        assert_eq!(csr.value_by_endpoint(0, 1), Some((0xDEAD_BEEF, true)));
        assert!(csr.set_value_by_endpoint(0, 2, Some(7)));
        assert_eq!(csr.value_by_endpoint(0, 2), Some((7, true)));
    }

    #[test]
    fn rank_and_duplicate_are_rejected() {
        let mut csr = BundledCsr::with_overflow_chunk_edges(8, 0, 4);
        assert!(csr
            .insert_edge_with_value(0, VertexId::edge_endpoint_key(1, 3), EdgeId(10), Some(1))
            .is_err());
        csr.insert_edge_with_value(0, dst(1), EdgeId(10), Some(1))
            .expect("first insert");
        assert!(csr.insert_edge(0, dst(1), EdgeId(11), 0).is_err());
    }

    #[test]
    fn delete_clears_valid_and_revert_with_value_restores() {
        let mut csr = BundledCsr::with_overflow_chunk_edges(8, 0, 4);
        csr.insert_edge_with_value(0, dst(1), EdgeId(10), Some(99))
            .expect("insert");
        let (position, _) = csr.locate_edge(0, EdgeId(10)).expect("located");
        assert!(csr.delete_edge(0, EdgeId(10), 5).expect("deleted"));
        assert_eq!(csr.value_by_edge_id(0, EdgeId(10)), None);
        assert_eq!(csr.value_at_position(0, position), Some((99, false)));
        assert!(csr.revert_delete_at_position_with_value(0, position, EdgeId(10), 5, Some(99)));
        assert_eq!(csr.value_by_edge_id(0, EdgeId(10)), Some((99, true)));
    }

    #[test]
    fn overflow_values_stay_aligned() {
        let mut csr = BundledCsr::with_overflow_chunk_edges(2, 0, 1);
        for i in 0..10u32 {
            csr.insert_edge_with_value(0, dst(100 + i), EdgeId(i as u64), Some(i as u64 * 10))
                .expect("overflow insert");
        }
        assert_eq!(csr.edge_count(), 10);
        for i in 0..10u32 {
            assert_eq!(
                csr.value_by_edge_id(0, EdgeId(i as u64)),
                Some((i as u64 * 10, true)),
                "overflow value drift at edge {}",
                i
            );
        }
        assert!(csr.set_value_by_edge_id(0, EdgeId(3), Some(0xFFFF)));
        assert_eq!(csr.value_by_edge_id(0, EdgeId(3)), Some((0xFFFF, true)));
        assert!(csr.rollback_insert(0, EdgeId(0)));
        assert_eq!(csr.edge_count(), 9);
        for i in 1..10u32 {
            let expect = if i == 3 { 0xFFFF } else { i as u64 * 10 };
            assert_eq!(
                csr.value_by_edge_id(0, EdgeId(i as u64)),
                Some((expect, true)),
                "value drift after physical remove at edge {}",
                i
            );
        }
    }

    #[test]
    fn dump_load_preserves_values() {
        let mut csr = BundledCsr::with_overflow_chunk_edges(4, 0, 1);
        for i in 0..6u32 {
            let value = if i % 2 == 0 { Some(i as u64) } else { None };
            csr.insert_edge_with_value(0, dst(10 + i), EdgeId(i as u64), value)
                .expect("insert");
        }
        let bytes = csr.dump();
        let mut loaded = BundledCsr::new();
        loaded.load(&bytes).expect("load roundtrip");
        assert_eq!(loaded.edge_count(), 6);
        for i in 0..6u32 {
            let expect = if i % 2 == 0 {
                Some((i as u64, true))
            } else {
                Some((0, false))
            };
            assert_eq!(loaded.value_by_edge_id(0, EdgeId(i as u64)), expect);
        }
        assert!(loaded.load(&bytes[..bytes.len() - 1]).is_err());
        let mut trailing = bytes.clone();
        trailing.push(0xAA);
        assert!(loaded.load(&trailing).is_err());
    }

    #[test]
    fn scalar_codec_roundtrip() {
        use graphdb_core::DataType;
        let cases = vec![
            (Value::Bool(true), DataType::Bool),
            (Value::Int(-12345), DataType::Int),
            (Value::BigInt(i64::MIN + 7), DataType::BigInt),
            (Value::Double(1.5), DataType::Double),
        ];
        for (value, dt) in cases {
            assert_eq!(decode_scalar(encode_scalar(&value), &dt), value);
        }
    }

    #[test]
    fn borrowed_walks_agree_without_materializing() {
        let mut csr = BundledCsr::with_overflow_chunk_edges(2, 0, 1);
        for i in 0..6u32 {
            let value = if i % 2 == 0 { Some(i as u64) } else { None };
            csr.insert_edge_with_value(0, dst(10 + i), EdgeId(i as u64), value)
                .expect("insert");
        }
        // Borrowed visitor walk over primary plus overflow.
        let mut visited = Vec::new();
        csr.visit_physical(0, |nbr| {
            visited.push((nbr.edge_id, nbr.endpoint));
            true
        });
        // Borrowed row iterator over the same row.
        let iterated: Vec<_> = csr
            .iter_row(0)
            .map(|nbr| (nbr.edge_id, nbr.endpoint))
            .collect();
        assert_eq!(visited, iterated);
        // Value-carrying walk pairs each entry with its inline value.
        let mut valued = Vec::new();
        csr.visit_physical_with_values(0, |nbr, value| {
            valued.push((nbr.edge_id, value));
            true
        });
        let expect: Vec<_> = (0..6u32)
            .map(|i| {
                let value = if i % 2 == 0 { Some(i as u64) } else { None };
                (EdgeId(i as u64), value)
            })
            .collect();
        assert_eq!(valued, expect);
    }
}
