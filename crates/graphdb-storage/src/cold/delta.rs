//! Cold snapshot delta (CDC) support.
//!
//! A `ColdDelta` captures the difference between two point-in-time edge
//! snapshots of the same label: edges added, edges removed, and property
//! updates. Deltas are self-contained (added/updated edge properties travel
//! inline), so a base snapshot plus a delta chain can reconstruct any later
//! state without re-exporting the full table.
//!
//! File format (`.lkcd`):
//! ```text
//! [4]  Magic "LKCD"
//! [4]  Version (u32 LE)
//! [8]  Base snapshot timestamp (u64 LE)
//! [8]  Delta timestamp (u64 LE)
//! [4]  Label ID (u32 LE)
//! [8]  Added edges count (u64 LE)
//! --- per added edge ---
//! [4]  src_internal (u32 LE)
//! [1+len] dst VertexId bytes (len-prefixed)
//! [8]  edge_id (u64 LE)
//! [8]  create timestamp (u64 LE)
//! [4]  property count (u32 LE)
//! --- per property ---
//! [4]  name length (u32 LE) + name bytes
//! [1]  value type tag + payload
//! [8]  Removed edges count (u64 LE)
//! --- per removed edge ---
//! [4]  src_internal (u32 LE)
//! [1+len] dst VertexId bytes (len-prefixed, encodes dst + rank)
//! [8]  Property updates count (u64 LE)
//! --- per update ---
//! [4]  src_internal (u32 LE)
//! [1+len] dst VertexId bytes
//! [4]  property count (u32 LE)
//! --- per property ---
//! [4]  name length (u32 LE) + name bytes
//! [1]  value type tag + payload
//! [4]  CRC32 of all preceding bytes
//! ```

use std::collections::HashMap;
use std::path::Path;

use crate::edge::edge_table::core::TimeTravelEdgeStore;
use crate::edge::{Csr, CsrBase, Nbr};
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};

use super::ColdSnapshot;

pub const COLD_DELTA_MAGIC: [u8; 4] = *b"LKCD";
pub const COLD_DELTA_VERSION: u32 = 1;

/// A newly added edge: the destination endpoint encodes `(dst, rank)` exactly
/// like the CSR neighbor keys, and the properties are stored inline so the
/// delta is self-contained.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaAddedEdge {
    pub src_internal: u32,
    pub neighbor: VertexId,
    pub edge_id: EdgeId,
    pub timestamp: Timestamp,
    pub properties: Vec<(String, Value)>,
}

/// An edge present in the base snapshot but absent in the newer one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeltaRemovedEdge {
    pub src_internal: u32,
    pub neighbor: VertexId,
}

/// An edge present in both snapshots whose property payload changed.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaPropertyUpdate {
    pub src_internal: u32,
    pub neighbor: VertexId,
    pub properties: Vec<(String, Value)>,
}

/// Difference between two cold snapshots of the same edge label.
#[derive(Debug, Clone, PartialEq)]
pub struct ColdDelta {
    pub base_ts: Timestamp,
    pub delta_ts: Timestamp,
    pub label: LabelId,
    pub added: Vec<DeltaAddedEdge>,
    pub removed: Vec<DeltaRemovedEdge>,
    pub property_updates: Vec<DeltaPropertyUpdate>,
}

impl ColdDelta {
    pub fn new(base_ts: Timestamp, delta_ts: Timestamp, label: LabelId) -> Self {
        Self {
            base_ts,
            delta_ts,
            label,
            added: Vec::new(),
            removed: Vec::new(),
            property_updates: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.property_updates.is_empty()
    }

    /// Net edge-count change when applied to the base snapshot.
    pub fn net_edge_change(&self) -> i64 {
        self.added.len() as i64 - self.removed.len() as i64
    }

    /// Compute the difference between two snapshots of the same label.
    ///
    /// `latest` must not be older than `base`. Edges are matched by their
    /// `(src_internal, encoded endpoint)` key, so insertions and deletions
    /// (including rank changes) are captured structurally.
    pub fn build(base: &ColdSnapshot, latest: &ColdSnapshot) -> StorageResult<Self> {
        if base.label() != latest.label() {
            return Err(StorageError::invalid_operation(format!(
                "delta requires same label: base={}, latest={}",
                base.label(),
                latest.label()
            )));
        }
        if latest.snapshot_ts() < base.snapshot_ts() {
            return Err(StorageError::invalid_operation(format!(
                "delta target {} is older than base {}",
                latest.snapshot_ts(),
                base.snapshot_ts()
            )));
        }
        if base.snapshot_ts() == latest.snapshot_ts() && !base.identical_to(latest) {
            return Err(StorageError::invalid_operation(
                "delta requires distinct snapshot timestamps",
            ));
        }

        let mut delta = Self::new(base.snapshot_ts(), latest.snapshot_ts(), base.label());

        let latest_map = Self::edge_index(latest);
        let base_map = Self::edge_index(base);

        // Removed + property-updated edges, iterating the base in CSR order.
        let base_cap = base.vertex_capacity();
        for src in 0..base_cap {
            let src_u32 = src as u32;
            for nbr in base.out_csr().edges_of(src_u32) {
                let key = (src_u32, nbr.neighbor);
                match latest_map.get(&key) {
                    None => delta.removed.push(DeltaRemovedEdge {
                        src_internal: src_u32,
                        neighbor: nbr.neighbor,
                    }),
                    Some(latest_nbr) => {
                        // Property payloads may change in place at the same
                        // offset, so compare the values, not the offsets.
                        let base_props = base
                            .properties()
                            .read_properties_by_edge_id(nbr.edge_id)
                            .unwrap_or_default();
                        let latest_props = latest
                            .properties()
                            .read_properties_by_edge_id(latest_nbr.edge_id)
                            .unwrap_or_default();
                        if base_props != latest_props {
                            delta.property_updates.push(DeltaPropertyUpdate {
                                src_internal: src_u32,
                                neighbor: nbr.neighbor,
                                properties: latest_props,
                            });
                        }
                    }
                }
            }
        }

        // Added edges: latest rows absent from the base.
        let latest_cap = latest.vertex_capacity();
        for src in 0..latest_cap {
            let src_u32 = src as u32;
            for nbr in latest.out_csr().edges_of(src_u32) {
                let key = (src_u32, nbr.neighbor);
                if base_map.contains_key(&key) {
                    continue;
                }
                let properties = latest
                    .properties()
                    .read_properties_by_edge_id(nbr.edge_id)
                    .unwrap_or_default();
                delta.added.push(DeltaAddedEdge {
                    src_internal: src_u32,
                    neighbor: nbr.neighbor,
                    edge_id: nbr.edge_id,
                    timestamp: nbr.timestamp,
                    properties,
                });
            }
        }

        Ok(delta)
    }

    /// Serialize this delta into a `.lkcd` file with a CRC32 footer.
    pub fn write<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let buf = self.encode()?;
        std::fs::write(path.as_ref(), &buf)
            .map_err(|e| StorageError::io_error(format!("failed to write delta file: {}", e)))
    }

    /// Load a delta from a `.lkcd` file, verifying magic and CRC32.
    pub fn open<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        let data = std::fs::read(path.as_ref())
            .map_err(|e| StorageError::io_error(format!("failed to read delta file: {}", e)))?;
        Self::from_bytes(&data)
    }

    pub fn from_bytes(data: &[u8]) -> StorageResult<Self> {
        let mut pos = 0usize;
        let mut read_arr = |n: usize| -> StorageResult<Vec<u8>> {
            if pos + n > data.len() {
                return Err(StorageError::deserialize_error("ColdDelta data truncated"));
            }
            let v = data[pos..pos + n].to_vec();
            pos += n;
            Ok(v)
        };
        let read_u32 = |pos: &mut usize| -> StorageResult<u32> {
            if *pos + 4 > data.len() {
                return Err(StorageError::deserialize_error("ColdDelta truncated (u32)"));
            }
            let v = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
            *pos += 4;
            Ok(v)
        };
        let read_u64 = |pos: &mut usize| -> StorageResult<u64> {
            if *pos + 8 > data.len() {
                return Err(StorageError::deserialize_error("ColdDelta truncated (u64)"));
            }
            let v = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(v)
        };
        let read_vertex = |pos: &mut usize| -> StorageResult<VertexId> {
            let len = read_u32(pos)? as usize;
            if *pos + len > data.len() {
                return Err(StorageError::deserialize_error(
                    "ColdDelta truncated (vertex id)",
                ));
            }
            let id = VertexId::from_bytes(data[*pos..*pos + len].to_vec());
            *pos += len;
            Ok(id)
        };
        let read_props = |pos: &mut usize| -> StorageResult<Vec<(String, Value)>> {
            let count = read_u32(pos)? as usize;
            let mut props = Vec::with_capacity(count);
            for _ in 0..count {
                let name_len = read_u32(pos)? as usize;
                if *pos + name_len > data.len() {
                    return Err(StorageError::deserialize_error(
                        "ColdDelta truncated (prop name)",
                    ));
                }
                let name = String::from_utf8_lossy(&data[*pos..*pos + name_len]).into_owned();
                *pos += name_len;
                let value = decode_value(data, pos)?;
                props.push((name, value));
            }
            Ok(props)
        };

        if read_arr(4)? != COLD_DELTA_MAGIC {
            return Err(StorageError::deserialize_error("invalid ColdDelta magic"));
        }
        let version = read_u32(&mut pos)?;
        if version != COLD_DELTA_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported ColdDelta version: {}",
                version
            )));
        }

        let base_ts = read_u64(&mut pos)?;
        let delta_ts = read_u64(&mut pos)?;
        let label = read_u32(&mut pos)?;

        let mut delta = Self::new(base_ts, delta_ts, label);

        let added_count = read_u64(&mut pos)? as usize;
        delta.added.reserve(added_count);
        for _ in 0..added_count {
            let src_internal = read_u32(&mut pos)?;
            let neighbor = read_vertex(&mut pos)?;
            let edge_id = EdgeId::new(read_u64(&mut pos)?);
            let timestamp = read_u64(&mut pos)?;
            let properties = read_props(&mut pos)?;
            delta.added.push(DeltaAddedEdge {
                src_internal,
                neighbor,
                edge_id,
                timestamp,
                properties,
            });
        }

        let removed_count = read_u64(&mut pos)? as usize;
        delta.removed.reserve(removed_count);
        for _ in 0..removed_count {
            let src_internal = read_u32(&mut pos)?;
            let neighbor = read_vertex(&mut pos)?;
            delta.removed.push(DeltaRemovedEdge {
                src_internal,
                neighbor,
            });
        }

        let update_count = read_u64(&mut pos)? as usize;
        delta.property_updates.reserve(update_count);
        for _ in 0..update_count {
            let src_internal = read_u32(&mut pos)?;
            let neighbor = read_vertex(&mut pos)?;
            let properties = read_props(&mut pos)?;
            delta.property_updates.push(DeltaPropertyUpdate {
                src_internal,
                neighbor,
                properties,
            });
        }

        // CRC32 verification
        if pos + 4 > data.len() {
            return Err(StorageError::deserialize_error("ColdDelta missing CRC"));
        }
        let stored_crc = read_u32(&mut pos)?;
        let computed_crc = crc32fast::hash(&data[..pos - 4]);
        if stored_crc != computed_crc {
            return Err(StorageError::deserialize_error(format!(
                "ColdDelta CRC mismatch: stored={:#x}, computed={:#x}",
                stored_crc, computed_crc
            )));
        }

        Ok(delta)
    }

    fn encode(&self) -> StorageResult<Vec<u8>> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&COLD_DELTA_MAGIC);
        buf.extend_from_slice(&COLD_DELTA_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.base_ts.to_le_bytes());
        buf.extend_from_slice(&self.delta_ts.to_le_bytes());
        buf.extend_from_slice(&self.label.to_le_bytes());

        buf.extend_from_slice(&(self.added.len() as u64).to_le_bytes());
        for edge in &self.added {
            buf.extend_from_slice(&edge.src_internal.to_le_bytes());
            write_vertex_id(&mut buf, edge.neighbor);
            buf.extend_from_slice(&edge.edge_id.as_u64().to_le_bytes());
            buf.extend_from_slice(&edge.timestamp.to_le_bytes());
            encode_props(&mut buf, &edge.properties)?;
        }

        buf.extend_from_slice(&(self.removed.len() as u64).to_le_bytes());
        for edge in &self.removed {
            buf.extend_from_slice(&edge.src_internal.to_le_bytes());
            write_vertex_id(&mut buf, edge.neighbor);
        }

        buf.extend_from_slice(&(self.property_updates.len() as u64).to_le_bytes());
        for update in &self.property_updates {
            buf.extend_from_slice(&update.src_internal.to_le_bytes());
            write_vertex_id(&mut buf, update.neighbor);
            encode_props(&mut buf, &update.properties)?;
        }

        let checksum = crc32fast::hash(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());
        Ok(buf)
    }

    /// Build an index of `(src_internal, neighbor bytes)` -> `ImmutableNbr`
    /// for structural edge matching.
    fn edge_index(snapshot: &ColdSnapshot) -> HashMap<(u32, VertexId), crate::edge::ImmutableNbr> {
        let mut index = HashMap::new();
        let cap = snapshot.vertex_capacity();
        for src in 0..cap {
            let src_u32 = src as u32;
            for nbr in snapshot.out_csr().edges_of(src_u32) {
                index.insert((src_u32, nbr.neighbor), *nbr);
            }
        }
        index
    }
}

impl ColdSnapshot {
    /// Compute the delta from `self` (base) to `latest` (newer state).
    pub fn diff(&self, latest: &ColdSnapshot) -> StorageResult<ColdDelta> {
        ColdDelta::build(self, latest)
    }

    /// Merge a delta into this snapshot, producing a new snapshot at the
    /// delta's timestamp.
    ///
    /// Removed edges are dropped, property updates are applied through the
    /// base property table (the updated row is tombstoned and reinserted),
    /// and added edges get fresh property offsets. A property index is
    /// rebuilt when the base carries one. The result is purely in-memory;
    /// call `persist`-style export to write it to a file.
    pub fn apply_delta(&self, delta: &ColdDelta) -> StorageResult<Self> {
        if delta.label != self.label() {
            return Err(StorageError::invalid_operation(format!(
                "delta label {} does not match snapshot label {}",
                delta.label,
                self.label()
            )));
        }
        if delta.delta_ts < self.snapshot_ts() {
            return Err(StorageError::invalid_operation(format!(
                "delta timestamp {} predates snapshot {}",
                delta.delta_ts,
                self.snapshot_ts()
            )));
        }

        let removed: HashMap<(u32, VertexId), ()> = delta
            .removed
            .iter()
            .map(|r| ((r.src_internal, r.neighbor), ()))
            .collect();
        let updates: HashMap<(u32, VertexId), &DeltaPropertyUpdate> = delta
            .property_updates
            .iter()
            .map(|u| ((u.src_internal, u.neighbor), u))
            .collect();

        let mut properties = self.properties().clone();
        let mut out_entries: Vec<(u32, Nbr, Timestamp)> = Vec::new();

        // Base edges minus removals, with property updates applied.
        let base_cap = self.vertex_capacity();
        for src in 0..base_cap {
            let src_u32 = src as u32;
            for e in self.out_csr().edges_of(src_u32) {
                let key = (src_u32, e.neighbor);
                if removed.contains_key(&key) {
                    continue;
                }
                let nbr = Nbr::new(e.neighbor, e.edge_id);
                if let Some(update) = updates.get(&key) {
                    if let Some(offset) = properties.get_offset_by_edge_id(nbr.edge_id) {
                        properties.update(offset, &update.properties, delta.delta_ts)?;
                    }
                }
                out_entries.push((src_u32, nbr, e.timestamp));
            }
        }

        // Added edges: properties go into the merged table, offsets remapped.
        for edge in &delta.added {
            if !edge.properties.is_empty() {
                properties.insert_with_edge_id(edge.edge_id, &edge.properties, edge.timestamp)?;
            };
            out_entries.push((
                edge.src_internal,
                Nbr::new(edge.neighbor, edge.edge_id),
                edge.timestamp,
            ));
        }

        // Rebuild both directions: in-CSR rows are the dst endpoints with the
        // encoded src endpoints as neighbors.
        let mut in_rows: Vec<(u32, Nbr, Timestamp)> = Vec::with_capacity(out_entries.len());
        let mut max_row = self.vertex_capacity();
        for (src, nbr, create_ts) in &out_entries {
            max_row = max_row.max(*src as usize + 1);
            let (dst_vid, rank) = TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
            if let Some(dst) = dst_vid.as_int64() {
                let dst_key = TimeTravelEdgeStore::edge_endpoint_key(*src, rank);
                let dst_row = dst as u32;
                max_row = max_row.max(dst_row as usize + 1);
                in_rows.push((
                    dst_row,
                    Nbr::new(dst_key, nbr.edge_id),
                    *create_ts,
                ));
            }
        }

        let out_csr = Csr::from_nbr_entries(&out_entries, max_row);
        let in_csr = Csr::from_nbr_entries(&in_rows, max_row);
        let edge_count = out_csr.edge_count();

        // Rebuild the property index when the base carried one.
        let property_index = if let Some(index) = self.property_index() {
            let exported = crate::edge::edge_table::snapshot::ExportedEdgeSnapshot {
                snapshot_ts: delta.delta_ts,
                label: self.label(),
                out_csr: out_csr.clone(),
                in_csr: in_csr.clone(),
                properties: properties.clone(),
                schema: self.schema().clone(),
            };
            let names = index.indexed_property_names();
            Some(crate::cold::ColdPropertyIndex::build(&exported, &names))
        } else {
            None
        };

        let mut snapshot = Self::from_parts(
            delta.delta_ts,
            self.label(),
            edge_count,
            max_row,
            out_csr,
            in_csr,
            properties,
            self.schema().clone(),
            property_index,
        );
        snapshot.with_path(self.backing_path().map(|p| p.to_path_buf()));
        Ok(snapshot)
    }
}

fn write_vertex_id(out: &mut Vec<u8>, id: VertexId) {
    let bytes = id.as_bytes();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Property value type tags for the delta codec. Mirrors the scalar subset
/// stored by `PropertyTable` (Bool/SmallInt/Int/BigInt/Float/Double/String/
/// Date). Unsupported value types are rejected at encode time.
fn encode_value(buf: &mut Vec<u8>, value: &Value) -> StorageResult<()> {
    match value {
        Value::Bool(b) => {
            buf.push(0);
            buf.push(*b as u8);
        }
        Value::SmallInt(i) => {
            buf.push(1);
            buf.extend_from_slice(&i.to_le_bytes());
        }
        Value::Int(i) => {
            buf.push(2);
            buf.extend_from_slice(&i.to_le_bytes());
        }
        Value::BigInt(i) => {
            buf.push(3);
            buf.extend_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            buf.push(4);
            buf.extend_from_slice(&f.to_le_bytes());
        }
        Value::Double(d) => {
            buf.push(5);
            buf.extend_from_slice(&d.to_le_bytes());
        }
        Value::String(s) => {
            buf.push(6);
            let bytes = s.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(bytes);
        }
        Value::FixedString(data) => {
            buf.push(6);
            let bytes = data.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(bytes);
        }
        Value::Date(d) => {
            buf.push(7);
            buf.extend_from_slice(&d.year.to_le_bytes());
            buf.extend_from_slice(&d.month.to_le_bytes());
            buf.extend_from_slice(&d.day.to_le_bytes());
        }
        other => {
            return Err(StorageError::serialize_error(format!(
                "delta codec does not support value type {}",
                other.get_type()
            )));
        }
    }
    Ok(())
}

fn decode_value(data: &[u8], pos: &mut usize) -> StorageResult<Value> {
    if *pos >= data.len() {
        return Err(StorageError::deserialize_error(
            "ColdDelta truncated (value tag)",
        ));
    }
    let tag = data[*pos];
    *pos += 1;
    let read_exact = |pos: &mut usize, n: usize| -> StorageResult<Vec<u8>> {
        if *pos + n > data.len() {
            return Err(StorageError::deserialize_error(
                "ColdDelta truncated (value payload)",
            ));
        }
        let v = data[*pos..*pos + n].to_vec();
        *pos += n;
        Ok(v)
    };
    Ok(match tag {
        0 => Value::Bool(read_exact(pos, 1)?[0] != 0),
        1 => Value::SmallInt(i16::from_le_bytes(read_exact(pos, 2)?.try_into().unwrap())),
        2 => Value::Int(i32::from_le_bytes(read_exact(pos, 4)?.try_into().unwrap())),
        3 => Value::BigInt(i64::from_le_bytes(read_exact(pos, 8)?.try_into().unwrap())),
        4 => Value::Float(f32::from_le_bytes(read_exact(pos, 4)?.try_into().unwrap())),
        5 => Value::Double(f64::from_le_bytes(read_exact(pos, 8)?.try_into().unwrap())),
        6 => {
            let len = u32::from_le_bytes(read_exact(pos, 4)?.try_into().unwrap()) as usize;
            let bytes = read_exact(pos, len)?;
            Value::string(String::from_utf8_lossy(&bytes))
        }
        7 => {
            let year = i32::from_le_bytes(read_exact(pos, 4)?.try_into().unwrap());
            let month = u32::from_le_bytes(read_exact(pos, 4)?.try_into().unwrap());
            let day = u32::from_le_bytes(read_exact(pos, 4)?.try_into().unwrap());
            Value::Date(graphdb_core::DateValue { year, month, day })
        }
        other => {
            return Err(StorageError::deserialize_error(format!(
                "unknown ColdDelta value tag: {}",
                other
            )));
        }
    })
}

fn encode_props(buf: &mut Vec<u8>, props: &[(String, Value)]) -> StorageResult<()> {
    buf.extend_from_slice(&(props.len() as u32).to_le_bytes());
    for (name, value) in props {
        let name_bytes = name.as_bytes();
        buf.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(name_bytes);
        encode_value(buf, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cold::ColdSnapshot;
    use crate::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::types::StoragePropertyDef;
    use graphdb_core::Value;

    fn make_table() -> TimeTravelEdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![
                StoragePropertyDef::new(
                    "weight".to_string(),
                    graphdb_core::types::DataType::Double,
                ),
                StoragePropertyDef::new("name".to_string(), graphdb_core::types::DataType::String),
            ],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    fn snapshot_at(table: &TimeTravelEdgeStore, ts: u64) -> ColdSnapshot {
        let dir = tempfile::tempdir().unwrap();
        let exported = table.export_snapshot(ts).unwrap();
        ColdSnapshot::create(&exported, dir.path().join(format!("s{}.lkcs", ts))).unwrap()
    }

    #[test]
    fn test_delta_build_apply_add_remove_update() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();

        let base = snapshot_at(&table, 100);

        table
            .insert_edge(3, 4, 0, &[("weight".to_string(), Value::Double(3.0))], 200)
            .unwrap();
        table
            .update_edge_property(0, 1, 0, "weight", &Value::Double(1.5), 200)
            .unwrap();
        table.delete_edge(0, 2, 0, 200).unwrap();

        let latest = snapshot_at(&table, 200);

        let delta = base.diff(&latest).unwrap();
        assert_eq!(delta.base_ts, 100);
        assert_eq!(delta.delta_ts, 200);
        assert_eq!(delta.added.len(), 1);
        assert_eq!(delta.removed.len(), 1);
        assert_eq!(delta.property_updates.len(), 1);
        assert_eq!(delta.net_edge_change(), 0);

        let merged = base.apply_delta(&delta).unwrap();
        assert_eq!(merged.snapshot_ts(), 200);
        assert_eq!(merged.edge_count(), 2);
        // 0->1 survives with updated weight
        let nbr = merged
            .get_edge_to_dst(0, 1)
            .expect("0->1 must survive update");
        let props = merged
            .properties()
            .read_properties_by_edge_id(nbr.edge_id)
            .unwrap();
        assert!(props.contains(&("weight".to_string(), Value::Double(1.5))));
        // 0->2 removed
        assert!(merged.get_edge_to_dst(0, 2).is_none());
        // 3->4 added
        assert!(merged.get_edge_to_dst(3, 4).is_some());
    }

    #[test]
    fn test_delta_roundtrip_file() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let base = snapshot_at(&table, 100);
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 200)
            .unwrap();
        let latest = snapshot_at(&table, 200);

        let delta = base.diff(&latest).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delta.lkcd");
        delta.write(&path).unwrap();

        let loaded = ColdDelta::open(&path).unwrap();
        assert_eq!(loaded, delta);
        assert_eq!(loaded.added.len(), 1);
        assert_eq!(loaded.added[0].src_internal, 0);
        let (dst, _) = TimeTravelEdgeStore::decode_edge_endpoint(loaded.added[0].neighbor);
        assert_eq!(dst, graphdb_core::types::VertexId::from_int64(2));

        // Corrupted payload must fail CRC verification.
        let mut corrupted = std::fs::read(&path).unwrap();
        if let Some(b) = corrupted.last_mut() {
            *b ^= 0xFF;
        }
        std::fs::write(&path, &corrupted).unwrap();
        assert!(ColdDelta::open(&path).is_err());
    }

    #[test]
    fn test_delta_empty_and_identity() {
        let table = make_table();
        let base = snapshot_at(&table, 100);
        let latest = snapshot_at(&table, 200);
        let delta = base.diff(&latest).unwrap();
        assert!(delta.is_empty());

        let merged = base.apply_delta(&delta).unwrap();
        assert_eq!(merged.edge_count(), 0);

        // Label mismatch rejected.
        let mut other = make_table();
        other.insert_edge(0, 1, 0, &[], 100).unwrap();
        let other_exported = other.export_snapshot(100).unwrap();
        let other_snapshot = ColdSnapshot::create(
            &other_exported,
            tempfile::tempdir().unwrap().path().join("o.lkcs"),
        )
        .unwrap();
        assert!(base.diff(&other_snapshot).is_err());
    }

    #[test]
    fn test_delta_rebuild_property_index() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let exported = table.export_snapshot(100).unwrap();
        let index = crate::cold::ColdPropertyIndex::build(&exported, &["weight".to_string()]);
        let base =
            ColdSnapshot::create_with_index(&exported, Some(index), dir.path().join("base.lkcs"))
                .unwrap();

        table
            .insert_edge(5, 6, 0, &[("weight".to_string(), Value::Double(9.0))], 200)
            .unwrap();
        let latest = snapshot_at(&table, 200);
        let delta = base.diff(&latest).unwrap();
        let merged = base.apply_delta(&delta).unwrap();

        assert!(merged.has_property_index());
        let index = merged.property_index().unwrap();
        let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
        let key = codec.encode(&Value::Double(9.0)).unwrap();
        let hits = index.lookup(
            "weight",
            &key,
            &graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&key),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].src_internal, 5);
        assert_eq!(hits[0].dst_internal, 6);
    }
}
