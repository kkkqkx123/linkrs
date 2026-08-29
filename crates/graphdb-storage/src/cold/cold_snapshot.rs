use std::collections::HashMap;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::edge::edge_table::core::TimeTravelEdgeStore;
use crate::edge::edge_table::remap::remap_immutable_csr;
use crate::edge::{Csr, CsrBase, EdgeRecord, EdgeSchema, Nbr, PropertyTable};
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};

use super::super::edge::edge_table::snapshot::ExportedEdgeSnapshot;

pub const COLD_SNAPSHOT_MAGIC: [u8; 4] = *b"LKCS";
/// Current on-disk format version.
pub const COLD_SNAPSHOT_VERSION: u32 = 1;
const HEADER_SIZE: usize = 36;

/// A scanned edge record from a ColdSnapshot.
/// Contains enough information to construct a full Edge.
#[derive(Debug, Clone)]
pub struct ColdEdgeRecord {
    pub src_internal: u32,
    pub dst_vid: VertexId,
    pub nbr: Nbr,
    pub rank: i64,
    pub properties: Option<Vec<(String, Value)>>,
}

/// A property-index hit: everything needed to reconstruct an edge whose
/// property value fell inside a lookup range.
#[derive(Debug, Clone, Copy)]
pub struct ColdIndexEntry {
    pub src_internal: u32,
    pub dst_internal: u32,
    pub rank: i64,
}

/// Ordered property index over a ColdSnapshot's edge rows.
///
/// For each indexed property name the entries are sorted by the
/// OrderedCodec-encoded value, so equality and range lookups use the same
/// encoded bounds as the hot per-table edge property index. Entries point at
/// (src_internal, dst_internal, rank, edge_id); the property payload
/// itself stays in the snapshot's property table.
#[derive(Debug, Clone, Default)]
pub struct ColdPropertyIndex {
    /// prop_name -> sorted (encoded_value, entry)
    entries: HashMap<String, Vec<(Vec<u8>, ColdIndexEntry)>>,
}

impl ColdPropertyIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an index over `prop_names` from an exported snapshot.
    pub fn build(exported: &ExportedEdgeSnapshot, prop_names: &[String]) -> Self {
        let mut index = Self::new();
        if prop_names.is_empty() {
            return index;
        }
        let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
        let cap = exported.out_csr.vertex_capacity();
        for src in 0..cap {
            let src_u32 = src as u32;
            for nbr in exported.out_csr.edges_of(src_u32) {
                let dst_internal = nbr.endpoint;
                let rank = nbr.rank;
                let Some(props) = exported.properties.read_properties_by_edge_id(nbr.edge_id)
                else {
                    continue;
                };
                for (name, value) in props {
                    if prop_names.iter().any(|n| n == &name) {
                        if let Ok(key) = codec.encode(&value) {
                            index.entries.entry(name).or_default().push((
                                key,
                                ColdIndexEntry {
                                    src_internal: src_u32,
                                    dst_internal,
                                    rank,
                                },
                            ));
                        }
                    }
                }
            }
        }
        for list in index.entries.values_mut() {
            list.sort_by(|a, b| a.0.cmp(&b.0));
        }
        index
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn indexed_property_names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn has_property(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Range lookup with the same encoded bounds as the hot edge property
    /// index (`value_lower <= key < value_upper`). An empty bound is
    /// unbounded on that side.
    pub fn lookup(
        &self,
        prop_name: &str,
        value_lower: &[u8],
        value_upper: &[u8],
    ) -> Vec<ColdIndexEntry> {
        let Some(entries) = self.entries.get(prop_name) else {
            return Vec::new();
        };
        let start = if value_lower.is_empty() {
            0
        } else {
            entries.partition_point(|(key, _)| key.as_slice() < value_lower)
        };
        let end = if value_upper.is_empty() {
            entries.len()
        } else {
            entries.partition_point(|(key, _)| key.as_slice() < value_upper)
        };
        if start >= end {
            return Vec::new();
        }
        entries[start..end].iter().map(|(_, e)| *e).collect()
    }

    /// Apply a vertex compaction internal-ID remap to every entry.
    pub fn remap_vertex_ids(
        &mut self,
        src_mapping: Option<&HashMap<u32, u32>>,
        dst_mapping: Option<&HashMap<u32, u32>>,
    ) {
        for list in self.entries.values_mut() {
            for (_, entry) in list.iter_mut() {
                if let Some(mapping) = src_mapping {
                    if let Some(&next) = mapping.get(&entry.src_internal) {
                        entry.src_internal = next;
                    }
                }
                if let Some(mapping) = dst_mapping {
                    if let Some(&next) = mapping.get(&entry.dst_internal) {
                        entry.dst_internal = next;
                    }
                }
            }
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        let mut names: Vec<&String> = self.entries.keys().collect();
        names.sort();
        for name in names {
            let name_bytes = name.as_bytes();
            buf.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(name_bytes);
            let list = &self.entries[name];
            buf.extend_from_slice(&(list.len() as u64).to_le_bytes());
            for (key, entry) in list {
                buf.extend_from_slice(&(key.len() as u32).to_le_bytes());
                buf.extend_from_slice(key);
                buf.extend_from_slice(&entry.src_internal.to_le_bytes());
                buf.extend_from_slice(&entry.dst_internal.to_le_bytes());
                buf.extend_from_slice(&entry.rank.to_le_bytes());
            }
        }
        buf
    }

    fn decode(data: &[u8]) -> StorageResult<Self> {
        let mut pos = 0usize;
        let read_u32 = |pos: &mut usize| -> StorageResult<u32> {
            if *pos + 4 > data.len() {
                return Err(StorageError::deserialize_error(
                    "cold index truncated (u32)",
                ));
            }
            let v = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
            *pos += 4;
            Ok(v)
        };
        let read_u64 = |pos: &mut usize| -> StorageResult<u64> {
            if *pos + 8 > data.len() {
                return Err(StorageError::deserialize_error(
                    "cold index truncated (u64)",
                ));
            }
            let v = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(v)
        };
        let read_bytes = |pos: &mut usize| -> StorageResult<Vec<u8>> {
            let len = read_u32(pos)? as usize;
            if *pos + len > data.len() {
                return Err(StorageError::deserialize_error(
                    "cold index truncated (bytes)",
                ));
            }
            let v = data[*pos..*pos + len].to_vec();
            *pos += len;
            Ok(v)
        };

        let prop_count = read_u32(&mut pos)? as usize;
        let mut entries: HashMap<String, Vec<(Vec<u8>, ColdIndexEntry)>> =
            HashMap::with_capacity(prop_count);
        for _ in 0..prop_count {
            let name_bytes = read_bytes(&mut pos)?;
            let name = String::from_utf8_lossy(&name_bytes).into_owned();
            let entry_count = read_u64(&mut pos)? as usize;
            let mut list = Vec::with_capacity(entry_count);
            for _ in 0..entry_count {
                let key = read_bytes(&mut pos)?;
                let src_internal = read_u32(&mut pos)?;
                let dst_internal = read_u32(&mut pos)?;
                let rank = read_u64(&mut pos)? as i64;
                list.push((
                    key,
                    ColdIndexEntry {
                        src_internal,
                        dst_internal,
                        rank,
                    },
                ));
            }
            entries.insert(name, list);
        }
        Ok(Self { entries })
    }
}

/// Read-only single-file columnar snapshot for cold analytics queries.
///
/// Contains CSR adjacency data (out/in), columnar property storage,
/// snapshot metadata, edge schema, and an optional ordered property index.
///
/// File format:
/// ```text
/// [4]  Magic "LKCS"
/// [4]  Version (u32 LE, 1)
/// [8]  Snapshot timestamp (u64 LE)
/// [8]  Edge count (u64 LE)
/// [4]  Label ID (u32 LE)
/// [8]  Vertex capacity (u64 LE)
/// --- sections ---
/// [8]  Out CSR length (u64 LE) + [N] out CSR (marker + raw/dict payload)
/// [8]  In CSR length (u64 LE) + [N] in CSR (marker + raw/dict payload)
/// [8]  Property table length (u64 LE) + [N] property data
///      (1-byte marker: 0x00 raw, 0x01 zstd-compressed)
/// [8]  Schema length (u64 LE) + [N] Schema data (JSON)
/// [8]  Index length (u64 LE, 0 = no index)
/// [N]  Index data (ColdPropertyIndex::encode)
/// [8]  Presence bitmap length (u64 LE, 0 = none)
/// [N]  Presence bitmap (u64 words, bit v = row v has out edges)
/// [4]  CRC32 of all preceding bytes
/// ```
///
/// Dict-encoded CSR payload:
/// ```text
/// [8]  Vertex capacity (u64 LE)
/// [8]  Edge count (u64 LE)
/// [8]  Dict size (u64 LE)
/// [N]  Dict entries: [1+len] vertex id bytes each
/// [8]  Offsets length (u64 LE)
/// [N]  Offsets (u32 LE, vertex_capacity + 1 entries)
/// [N]  Edge records: (dict_id u32, edge_id u64, ts u64)
/// ```
#[derive(Debug, Clone)]
pub struct ColdSnapshot {
    snapshot_ts: Timestamp,
    label: LabelId,
    edge_count: u64,
    vertex_capacity: usize,
    out_csr: Csr,
    in_csr: Csr,
    properties: PropertyTable,
    schema: EdgeSchema,
    property_index: Option<ColdPropertyIndex>,
    /// Bit v set = row v holds at least one out edge. Lets full scans skip
    /// empty rows without touching the CSR offsets.
    vertex_presence: Option<Vec<u64>>,
    /// Backing `.lkcs` file when opened from or created at a path.
    path: Option<PathBuf>,
}

impl ColdSnapshot {
    pub fn open<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        let file = std::fs::File::open(path.as_ref())
            .map_err(|e| StorageError::io_error(format!("failed to open snapshot: {}", e)))?;
        let mmap = unsafe {
            Mmap::map(&file)
                .map_err(|e| StorageError::io_error(format!("failed to mmap snapshot: {}", e)))?
        };
        let mut snapshot = Self::from_bytes(&mmap)?;
        snapshot.path = Some(path.as_ref().to_path_buf());
        drop(file);
        Ok(snapshot)
    }

    pub fn from_bytes(data: &[u8]) -> StorageResult<Self> {
        if data.len() < HEADER_SIZE + 4 {
            return Err(StorageError::deserialize_error(format!(
                "ColdSnapshot data too short: {} bytes",
                data.len()
            )));
        }

        let mut pos = 0usize;

        let magic = &data[pos..pos + 4];
        pos += 4;
        if magic != COLD_SNAPSHOT_MAGIC {
            return Err(StorageError::deserialize_error(format!(
                "invalid magic: {:?}",
                magic
            )));
        }

        let version = u32::from_le_bytes(read_arr::<4>(data, &mut pos));
        if version != COLD_SNAPSHOT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported ColdSnapshot version: {}",
                version
            )));
        }

        let snapshot_ts = u64::from_le_bytes(read_arr::<8>(data, &mut pos));
        let edge_count = u64::from_le_bytes(read_arr::<8>(data, &mut pos));
        let label = u32::from_le_bytes(read_arr::<4>(data, &mut pos));
        let vertex_capacity = u64::from_le_bytes(read_arr::<8>(data, &mut pos)) as usize;

        // Sections
        let out_data = read_section(data, &mut pos)?;
        let in_data = read_section(data, &mut pos)?;
        let prop_section = read_section(data, &mut pos)?;
        let schema_data = read_section(data, &mut pos)?;

        // Property section: 1-byte marker (raw / zstd) + payload.
        let (marker, prop_payload) = prop_section.split_first().ok_or_else(|| {
            StorageError::deserialize_error("ColdSnapshot property section empty")
        })?;
        let prop_data: Vec<u8> = match *marker {
            ZSTD_MARKER => zstd::decode_all(prop_payload).map_err(|e| {
                StorageError::deserialize_error(format!("zstd decompress failed: {}", e))
            })?,
            RAW_MARKER => prop_payload.to_vec(),
            other => {
                return Err(StorageError::deserialize_error(format!(
                    "unknown property marker: {:#x}",
                    other
                )));
            }
        };

        // Optional property index section (length 0 = no index)
        let index_len = u64::from_le_bytes(read_arr::<8>(data, &mut pos));
        let property_index = if index_len > 0 {
            if pos + index_len as usize > data.len() {
                return Err(StorageError::deserialize_error(
                    "ColdSnapshot index section exceeds file size",
                ));
            }
            let index_data = &data[pos..pos + index_len as usize];
            pos += index_len as usize;
            Some(ColdPropertyIndex::decode(index_data)?)
        } else {
            None
        };

        // Optional presence bitmap section (length 0 = none)
        let presence_len = u64::from_le_bytes(read_arr::<8>(data, &mut pos));
        let vertex_presence = if presence_len > 0 {
            if pos + presence_len as usize > data.len() {
                return Err(StorageError::deserialize_error(
                    "ColdSnapshot presence bitmap exceeds file size",
                ));
            }
            if !presence_len.is_multiple_of(8) {
                return Err(StorageError::deserialize_error(
                    "ColdSnapshot presence bitmap length is not word-aligned",
                ));
            }
            let words = presence_len as usize / 8;
            let mut bitmap = Vec::with_capacity(words);
            for _ in 0..words {
                bitmap.push(u64::from_le_bytes(read_arr::<8>(data, &mut pos)));
            }
            Some(bitmap)
        } else {
            None
        };

        // CRC32 verification
        let stored_crc = u32::from_le_bytes(read_arr::<4>(data, &mut pos));
        let computed_crc = crc32fast::hash(&data[..pos - 4]);
        if stored_crc != computed_crc {
            return Err(StorageError::deserialize_error(format!(
                "CRC mismatch: stored={:#x}, computed={:#x}",
                stored_crc, computed_crc
            )));
        }

        // Deserialize
        let out_csr = decode_csr_section(out_data)?;
        let in_csr = decode_csr_section(in_data)?;

        let mut properties = PropertyTable::new();
        properties.load(&prop_data)?;

        let schema_json = std::str::from_utf8(schema_data)
            .map_err(|e| StorageError::deserialize_error(format!("invalid schema utf-8: {}", e)))?;
        let schema: EdgeSchema = serde_json::from_str(schema_json)
            .map_err(|e| StorageError::deserialize_error(format!("invalid schema json: {}", e)))?;

        Ok(Self {
            snapshot_ts,
            label,
            edge_count,
            vertex_capacity,
            out_csr,
            in_csr,
            properties,
            schema,
            property_index,
            vertex_presence,
            path: None,
        })
    }

    pub fn create<P: AsRef<Path>>(exported: &ExportedEdgeSnapshot, path: P) -> StorageResult<Self> {
        Self::create_with_index(exported, None, path)
    }

    /// Create a snapshot file with an optional property index.
    pub fn create_with_index<P: AsRef<Path>>(
        exported: &ExportedEdgeSnapshot,
        index: Option<ColdPropertyIndex>,
        path: P,
    ) -> StorageResult<Self> {
        let buf = encode_snapshot(
            &exported.out_csr,
            &exported.in_csr,
            &exported.properties,
            &exported.schema,
            exported.snapshot_ts,
            exported.label,
            index.as_ref(),
        )?;

        std::fs::write(path.as_ref(), &buf)
            .map_err(|e| StorageError::io_error(format!("failed to write snapshot file: {}", e)))?;

        let mut snapshot = Self::from_bytes(&buf)?;
        snapshot.path = Some(path.as_ref().to_path_buf());
        Ok(snapshot)
    }

    /// Persist the current in-memory state back to the backing `.lkcs` file.
    ///
    /// Keeps the file consistent after an in-memory remap (e.g. vertex
    /// compaction). The file is a rebuildable cache, so a failure here only
    /// degrades to a stale file, never to incorrect queries.
    pub fn persist(&self) -> StorageResult<()> {
        let path = self.path.as_ref().ok_or_else(|| {
            StorageError::io_error("cold snapshot has no backing file to persist to")
        })?;
        let buf = encode_snapshot(
            &self.out_csr,
            &self.in_csr,
            &self.properties,
            &self.schema,
            self.snapshot_ts,
            self.label,
            self.property_index.as_ref(),
        )?;
        std::fs::write(path, &buf)
            .map_err(|e| StorageError::io_error(format!("failed to rewrite snapshot file: {}", e)))
    }

    /// Build a minimal empty snapshot file (no edges) and return the
    /// in-memory snapshot bound to that path. Used by tooling and tests that
    /// need a valid `.lkcs` file without an edge table.
    pub fn create_empty<P: AsRef<Path>>(
        label: LabelId,
        label_name: &str,
        ts: Timestamp,
        path: P,
    ) -> StorageResult<Self> {
        let schema = EdgeSchema {
            label_id: label,
            label_name: label_name.to_string(),
            src_label: 0,
            dst_label: 0,
            properties: Vec::new(),
            oe_strategy: crate::edge::EdgeStrategy::Multiple,
            ie_strategy: crate::edge::EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let exported = ExportedEdgeSnapshot {
            snapshot_ts: ts,
            label,
            out_csr: Csr::new(),
            in_csr: Csr::new(),
            properties: PropertyTable::new(),
            schema,
        };
        Self::create(&exported, path)
    }

    /// Serialize the current in-memory state to `path` (`.lkcs` format)
    /// and return a copy bound to that path.
    pub(crate) fn export_to_path<P: AsRef<Path>>(&self, path: P) -> StorageResult<Self> {
        let buf = encode_snapshot(
            &self.out_csr,
            &self.in_csr,
            &self.properties,
            &self.schema,
            self.snapshot_ts,
            self.label,
            self.property_index.as_ref(),
        )?;
        std::fs::write(path.as_ref(), &buf)
            .map_err(|e| StorageError::io_error(format!("failed to write snapshot file: {}", e)))?;
        let mut copy = Self::from_bytes(&buf)?;
        copy.path = Some(path.as_ref().to_path_buf());
        Ok(copy)
    }

    pub fn snapshot_ts(&self) -> Timestamp {
        self.snapshot_ts
    }

    /// Structural equality of the CSR data (edges, endpoints, offsets).
    /// Property payloads are compared through `read_properties` on demand by
    /// delta builders; this only guards the "identical timestamp" fast path.
    pub(crate) fn identical_to(&self, other: &Self) -> bool {
        if self.edge_count != other.edge_count || self.label != other.label {
            return false;
        }
        let cap = self.vertex_capacity.max(other.vertex_capacity);
        for src in 0..cap {
            let a = self.out_csr.edges_of(src as u32);
            let b = other.out_csr.edges_of(src as u32);
            if a.len() != b.len() || !a.iter().zip(b.iter()).all(|(x, y)| x == y) {
                return false;
            }
        }
        true
    }

    /// Reconstruct a snapshot from already-parsed parts. Used by the delta
    /// application path, which mutates a base snapshot in place of a fresh
    /// file read. The presence bitmap is rebuilt from the out CSR.
    #[allow(clippy::too_many_arguments)]
    #[doc(hidden)]
    pub(crate) fn from_parts(
        snapshot_ts: Timestamp,
        label: LabelId,
        edge_count: u64,
        vertex_capacity: usize,
        out_csr: Csr,
        in_csr: Csr,
        properties: PropertyTable,
        schema: EdgeSchema,
        property_index: Option<ColdPropertyIndex>,
    ) -> Self {
        let vertex_presence = {
            let bitmap = build_presence_bitmap(&out_csr);
            if bitmap.is_empty() {
                None
            } else {
                Some(bitmap)
            }
        };
        Self {
            snapshot_ts,
            label,
            edge_count,
            vertex_capacity,
            out_csr,
            in_csr,
            properties,
            schema,
            property_index,
            vertex_presence,
            path: None,
        }
    }

    /// Attach a backing file path (or clear it) after in-memory
    /// reconstruction such as delta application.
    pub(crate) fn with_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
    }

    /// Build an in-memory snapshot from an exported edge snapshot without
    /// touching the filesystem (used by delta computation).
    pub(crate) fn from_export(exported: &ExportedEdgeSnapshot) -> StorageResult<Self> {
        Ok(Self::from_parts(
            exported.snapshot_ts,
            exported.label,
            exported.out_csr.edge_count(),
            exported
                .out_csr
                .vertex_capacity()
                .max(exported.in_csr.vertex_capacity()),
            exported.out_csr.clone(),
            exported.in_csr.clone(),
            exported.properties.clone(),
            exported.schema.clone(),
            None,
        ))
    }

    pub fn label(&self) -> LabelId {
        self.label
    }

    /// Backing `.lkcs` file path when the snapshot was opened from or
    /// created at a path.
    pub(crate) fn backing_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    pub fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    pub fn out_csr(&self) -> &Csr {
        &self.out_csr
    }

    pub fn in_csr(&self) -> &Csr {
        &self.in_csr
    }

    pub fn properties(&self) -> &PropertyTable {
        &self.properties
    }

    pub fn schema(&self) -> &EdgeSchema {
        &self.schema
    }

    pub fn property_index(&self) -> Option<&ColdPropertyIndex> {
        self.property_index.as_ref()
    }

    /// Whether this snapshot carries a property index at all.
    pub fn has_property_index(&self) -> bool {
        self.property_index.as_ref().is_some_and(|i| !i.is_empty())
    }

    pub fn get_out_edges(&self, src: u32) -> Vec<Nbr> {
        self.out_csr
            .edges_of(src)
            .iter()
            .map(|e| Nbr::with_prop_offset(e.endpoint, e.rank, e.edge_id, e.prop_offset))
            .collect()
    }

    pub fn get_in_edges(&self, dst: u32) -> Vec<Nbr> {
        self.in_csr
            .edges_of(dst)
            .iter()
            .map(|e| Nbr::with_prop_offset(e.endpoint, e.rank, e.edge_id, e.prop_offset))
            .collect()
    }

    pub fn get_edge(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        self.out_csr
            .get_edge(src, dst)
            .map(|e| Nbr::with_prop_offset(e.endpoint, e.rank, e.edge_id, e.prop_offset))
    }

    /// Find an edge from `src` (internal CSR index) to `dst` (internal vertex id).
    ///
    /// The CSR stores neighbors as encoded `(dst_internal, rank)` keys, so the
    /// lookup decodes each neighbor before comparing with `dst`.
    pub fn get_edge_to_dst(&self, src: u32, dst: u32) -> Option<Nbr> {
        self.out_csr.edges_of(src).iter().find_map(|e| {
            let decoded = VertexId::from_int64(e.endpoint as i64);
            if decoded.as_int64() == Some(dst as i64) {
                Some(Nbr::with_prop_offset(
                    e.endpoint,
                    e.rank,
                    e.edge_id,
                    e.prop_offset,
                ))
            } else {
                None
            }
        })
    }

    pub fn degree(&self, src: u32) -> usize {
        self.out_csr.edges_of(src).len()
    }

    pub fn scan_edges(&self) -> Vec<ColdEdgeRecord> {
        let cap = self.vertex_capacity;
        let mut results = Vec::with_capacity(self.edge_count as usize);
        for src in 0..cap {
            if !self.row_has_edges(src) {
                continue;
            }
            let src_u32 = src as u32;
            for nbr in self.out_csr.edges_of(src_u32) {
                let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                results.push(ColdEdgeRecord {
                    src_internal: src as u32,
                    dst_vid,
                    nbr: Nbr::with_prop_offset(
                        nbr.endpoint,
                        nbr.rank,
                        nbr.edge_id,
                        nbr.prop_offset,
                    ),
                    rank,
                    properties: None,
                });
            }
        }
        results
    }

    /// Whether vertex row `row` holds at least one out edge, using the
    /// presence bitmap when available and the CSR offsets otherwise.
    pub fn row_has_edges(&self, row: usize) -> bool {
        if let Some(bitmap) = &self.vertex_presence {
            let word = row / 64;
            let bit = row % 64;
            bitmap.get(word).is_some_and(|w| (w & (1u64 << bit)) != 0)
        } else {
            !self.out_csr.edges_of(row as u32).is_empty()
        }
    }

    /// The out-row presence bitmap (bit v = row v has edges), when the
    /// snapshot file carried one.
    pub fn vertex_presence(&self) -> Option<&[u64]> {
        self.vertex_presence.as_deref()
    }

    pub fn nbr_to_edge_record(
        &self,
        nbr: &Nbr,
        src_vid: VertexId,
        dst_vid: VertexId,
    ) -> EdgeRecord {
        let rank = nbr.rank;
        let properties = self
            .properties
            .read_properties_by_edge_id(nbr.edge_id)
            .unwrap_or_default();
        EdgeRecord {
            src_vid,
            dst_vid,
            rank,
            properties,
        }
    }

    /// Rebuild both CSRs with translated rows/neighbors and a truncated row
    /// space (max edge-bearing row + 1) after a vertex compaction remap.
    ///
    /// `src_mapping` applies to out rows / in neighbors; `dst_mapping` to in
    /// rows / out neighbors (per-label internal ID spaces).
    ///
    /// Only the in-memory snapshot is updated; the backing `.lkcs` file is
    /// left untouched and should be re-exported to stay consistent.
    pub fn remap_vertex_ids(
        &mut self,
        src_mapping: Option<&HashMap<u32, u32>>,
        dst_mapping: Option<&HashMap<u32, u32>>,
    ) -> StorageResult<()> {
        let new_out = remap_immutable_csr(&self.out_csr, src_mapping, dst_mapping)?;
        let new_in = remap_immutable_csr(&self.in_csr, dst_mapping, src_mapping)?;

        let out_capacity = new_out.vertex_capacity();
        let in_capacity = new_in.vertex_capacity();
        self.out_csr = new_out;
        self.in_csr = new_in;
        self.vertex_capacity = out_capacity.max(in_capacity);
        self.edge_count = self.out_csr.edge_count();
        let bitmap = build_presence_bitmap(&self.out_csr);
        self.vertex_presence = if bitmap.is_empty() {
            None
        } else {
            Some(bitmap)
        };

        if let Some(index) = self.property_index.as_mut() {
            index.remap_vertex_ids(src_mapping, dst_mapping);
        }

        log::debug!(
            "ColdSnapshot[label={}] remapped vertex IDs (src_mapping={}, dst_mapping={}); capacity={}, edges={}",
            self.label,
            src_mapping.map(|m| m.len()).unwrap_or(0),
            dst_mapping.map(|m| m.len()).unwrap_or(0),
            self.vertex_capacity,
            self.edge_count
        );

        Ok(())
    }
}

fn read_arr<const N: usize>(data: &[u8], pos: &mut usize) -> [u8; N] {
    let arr: [u8; N] = data[*pos..*pos + N].try_into().unwrap();
    *pos += N;
    arr
}

fn read_section<'a>(data: &'a [u8], pos: &mut usize) -> StorageResult<&'a [u8]> {
    if *pos + 8 > data.len() {
        return Err(StorageError::deserialize_error(
            "unexpected end of section length",
        ));
    }
    let len = u64::from_le_bytes(read_arr::<8>(data, pos)) as usize;
    if *pos + len > data.len() {
        return Err(StorageError::deserialize_error(format!(
            "section data exceeds file: offset={}, len={}, file_size={}",
            *pos,
            len,
            data.len()
        )));
    }
    let section = &data[*pos..*pos + len];
    *pos += len;
    Ok(section)
}

fn write_section(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
    buf.extend_from_slice(data);
}

/// Serialize CSR/property data into the `.lkcs` file layout (header + sections + CRC).
fn encode_snapshot(
    out_csr: &Csr,
    in_csr: &Csr,
    properties: &PropertyTable,
    schema: &EdgeSchema,
    snapshot_ts: Timestamp,
    label: LabelId,
    index: Option<&ColdPropertyIndex>,
) -> StorageResult<Vec<u8>> {
    let out_data = encode_csr_section(out_csr);
    let in_data = encode_csr_section(in_csr);
    let prop_data = properties.dump();
    // zstd-compress the property section when it actually shrinks.
    let compressed_prop = zstd::encode_all(&prop_data[..], ZSTD_LEVEL)
        .map_err(|e| StorageError::serialize_error(format!("zstd compress failed: {}", e)))?;
    let (prop_marker, prop_payload) = if compressed_prop.len() < prop_data.len() {
        (ZSTD_MARKER, compressed_prop)
    } else {
        (RAW_MARKER, prop_data)
    };
    let schema_json =
        serde_json::to_string(schema).map_err(|e| StorageError::serialize_error(e.to_string()))?;
    let schema_bytes = schema_json.as_bytes();

    let presence = build_presence_bitmap(out_csr);

    let mut buf = Vec::new();

    buf.extend_from_slice(&COLD_SNAPSHOT_MAGIC);
    buf.extend_from_slice(&COLD_SNAPSHOT_VERSION.to_le_bytes());
    buf.extend_from_slice(&snapshot_ts.to_le_bytes());
    buf.extend_from_slice(&out_csr.edge_count().to_le_bytes());
    buf.extend_from_slice(&label.to_le_bytes());
    buf.extend_from_slice(&(out_csr.vertex_capacity() as u64).to_le_bytes());

    write_section(&mut buf, &out_data);
    write_section(&mut buf, &in_data);

    // Property section: length prefix covers marker + payload.
    let mut prop_section = Vec::with_capacity(1 + prop_payload.len());
    prop_section.push(prop_marker);
    prop_section.extend_from_slice(&prop_payload);
    write_section(&mut buf, &prop_section);

    write_section(&mut buf, schema_bytes);

    if let Some(index) = index {
        let index_data = index.encode();
        buf.extend_from_slice(&(index_data.len() as u64).to_le_bytes());
        buf.extend_from_slice(&index_data);
    } else {
        buf.extend_from_slice(&0u64.to_le_bytes());
    }

    buf.extend_from_slice(&((presence.len() * 8) as u64).to_le_bytes());
    for word in &presence {
        buf.extend_from_slice(&word.to_le_bytes());
    }

    let checksum = crc32fast::hash(&buf);
    buf.extend_from_slice(&checksum.to_le_bytes());

    Ok(buf)
}

const ZSTD_MARKER: u8 = 0x01;
const RAW_MARKER: u8 = 0x00;
const ZSTD_LEVEL: i32 = 3;

/// Build the out-row presence bitmap: bit v set = vertex row v has at least
/// one out edge. Rows past the CSR capacity are implicitly absent.
fn build_presence_bitmap(csr: &Csr) -> Vec<u64> {
    let capacity = csr.vertex_capacity();
    if capacity == 0 || csr.edge_count() == 0 {
        return Vec::new();
    }
    let mut bitmap = vec![0u64; capacity.div_ceil(64)];
    for v in 0..capacity {
        if !csr.edges_of(v as u32).is_empty() {
            let word = v / 64;
            let bit = v % 64;
            bitmap[word] |= 1u64 << bit;
        }
    }
    bitmap
}

/// Dict-encode a CSR: distinct endpoint `VertexId`s become u32 dictionary
/// references, so repeated neighbors cost 24 bytes instead of 37. The
/// section carries a format marker so the smaller of raw/dict encoding is
/// chosen per CSR:
/// ```text
/// [1] marker (0x00 raw Csr::dump, 0x01 dict-encoded)
/// [N] payload
/// ```
fn encode_csr_section(csr: &Csr) -> Vec<u8> {
    let raw_data = csr.dump();
    let dict_data = encode_csr_dict(csr);
    let mut buf = Vec::new();
    if dict_data.len() < raw_data.len() {
        buf.push(CSR_DICT);
        buf.extend_from_slice(&dict_data);
    } else {
        buf.push(CSR_RAW);
        buf.extend_from_slice(&raw_data);
    }
    buf
}

/// Decode a CSR section written by [`encode_csr_section`].
fn decode_csr_section(data: &[u8]) -> StorageResult<Csr> {
    let (marker, payload) = data
        .split_first()
        .ok_or_else(|| StorageError::deserialize_error("cold CSR section empty"))?;
    match *marker {
        CSR_RAW => {
            let mut csr = Csr::new();
            csr.load(payload)?;
            Ok(csr)
        }
        CSR_DICT => decode_csr_dict(payload),
        other => Err(StorageError::deserialize_error(format!(
            "unknown cold CSR marker: {:#x}",
            other
        ))),
    }
}

const CSR_RAW: u8 = 0x00;
const CSR_DICT: u8 = 0x01;

/// Dict-encode a CSR (payload of a `CSR_DICT` section).
fn encode_csr_dict(csr: &Csr) -> Vec<u8> {
    let capacity = csr.vertex_capacity();
    let edge_count = csr.edge_count();

    // Build the endpoint dictionary in order of first appearance.
    let mut dict: Vec<VertexId> = Vec::new();
    let mut dict_ids: HashMap<VertexId, u32> = HashMap::new();
    let mut edges: Vec<(u32, EdgeId, Timestamp)> = Vec::with_capacity(edge_count as usize);
    for v in 0..capacity {
        for nbr in csr.edges_of(v as u32) {
            let nbr_vid = nbr.to_vertex_id();
            let id = match dict_ids.get(&nbr_vid) {
                Some(&id) => id,
                None => {
                    let id = dict.len() as u32;
                    dict.push(nbr_vid);
                    dict_ids.insert(nbr_vid, id);
                    id
                }
            };
            edges.push((id, nbr.edge_id, nbr.timestamp));
        }
    }

    let mut buf = Vec::new();
    buf.extend_from_slice(&(capacity as u64).to_le_bytes());
    buf.extend_from_slice(&(edge_count).to_le_bytes());
    buf.extend_from_slice(&(dict.len() as u64).to_le_bytes());
    for id in &dict {
        let bytes = id.as_bytes();
        buf.push(bytes.len() as u8);
        buf.extend_from_slice(bytes);
    }
    // Offsets: capacity + 1 entries, relative to the edge records.
    let mut offsets = Vec::with_capacity(capacity + 1);
    let mut running = 0u32;
    offsets.push(0);
    for v in 0..capacity {
        running += csr.edges_of(v as u32).len() as u32;
        offsets.push(running);
    }
    buf.extend_from_slice(&(offsets.len() as u64).to_le_bytes());
    for offset in &offsets {
        buf.extend_from_slice(&offset.to_le_bytes());
    }
    for (dict_id, edge_id, ts) in &edges {
        buf.extend_from_slice(&dict_id.to_le_bytes());
        buf.extend_from_slice(&edge_id.as_u64().to_le_bytes());
        buf.extend_from_slice(&ts.to_le_bytes());
    }
    buf
}

/// Decode a dict-encoded CSR payload back into a `Csr`.
fn decode_csr_dict(data: &[u8]) -> StorageResult<Csr> {
    let mut pos = 0usize;
    let read_u64 = |pos: &mut usize| -> StorageResult<u64> {
        if *pos + 8 > data.len() {
            return Err(StorageError::deserialize_error(
                "cold CSR section truncated (u64)",
            ));
        }
        let v = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
        *pos += 8;
        Ok(v)
    };

    let capacity = read_u64(&mut pos)? as usize;
    let edge_count = read_u64(&mut pos)? as usize;
    let dict_len = read_u64(&mut pos)? as usize;
    let mut dict = Vec::with_capacity(dict_len);
    for _ in 0..dict_len {
        if pos >= data.len() {
            return Err(StorageError::deserialize_error(
                "cold CSR dict truncated (id length)",
            ));
        }
        let len = data[pos] as usize;
        pos += 1;
        if pos + len > data.len() {
            return Err(StorageError::deserialize_error(
                "cold CSR dict truncated (id bytes)",
            ));
        }
        dict.push(VertexId::from_bytes(data[pos..pos + len].to_vec()));
        pos += len;
    }

    let offsets_len = read_u64(&mut pos)? as usize;
    if offsets_len != capacity + 1 {
        return Err(StorageError::deserialize_error(format!(
            "cold CSR offsets length mismatch: expected {}, got {}",
            capacity + 1,
            offsets_len
        )));
    }
    if pos + offsets_len * 4 > data.len() {
        return Err(StorageError::deserialize_error(
            "cold CSR offsets truncated",
        ));
    }
    let mut offsets = Vec::with_capacity(offsets_len);
    for _ in 0..offsets_len {
        offsets.push(u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()));
        pos += 4;
    }
    if pos + edge_count * 16 > data.len() {
        return Err(StorageError::deserialize_error(
            "cold CSR edge records truncated",
        ));
    }

    // Expand dictionary references back into ImmutableNbr values.
    let mut entries: Vec<(u32, Nbr, Timestamp)> = Vec::with_capacity(edge_count);
    for v in 0..capacity {
        let start = offsets[v] as usize;
        let end = offsets[v + 1] as usize;
        for _ in start..end {
            let dict_id = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let edge_id = EdgeId::new(u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()));
            pos += 8;
            let ts = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let neighbor = *dict
                .get(dict_id as usize)
                .ok_or_else(|| StorageError::deserialize_error("cold CSR dict id out of range"))?;
            let (endpoint_vid, rank) = neighbor.decode_edge_endpoint();
            entries.push((
                v as u32,
                Nbr::with_prop_offset(
                    endpoint_vid.as_int64().unwrap_or(0) as u32,
                    rank,
                    edge_id,
                    crate::edge::property_schema::PROP_OFFSET_NONE,
                ),
                ts,
            ));
        }
    }
    Ok(Csr::from_nbr_entries(&entries, capacity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::EdgeId;
    use graphdb_core::Value;

    fn make_table() -> TimeTravelEdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                graphdb_core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_cold_snapshot_roundtrip() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
            .unwrap();

        let exported = table.export_snapshot(100).unwrap();
        assert_eq!(exported.out_csr.edge_count(), 2);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");

        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert_eq!(snapshot.edge_count(), 2);
        assert_eq!(snapshot.snapshot_ts(), 100);
        assert_eq!(snapshot.label(), 0);

        let loaded = ColdSnapshot::open(&path).unwrap();
        assert_eq!(loaded.edge_count(), 2);
        assert_eq!(loaded.snapshot_ts(), 100);
        assert_eq!(loaded.get_out_edges(0).len(), 2);
        assert_eq!(loaded.get_out_edges(1).len(), 0);
    }

    fn make_edge_key(endpoint: u32, rank: i64) -> VertexId {
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&(endpoint as i64).to_be_bytes());
        data.extend_from_slice(&rank.to_be_bytes());
        VertexId::from_bytes(data)
    }

    #[test]
    fn test_cold_snapshot_get_edge() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 50)
            .unwrap();
        table
            .insert_edge(5, 10, 0, &[("weight".to_string(), Value::Double(3.0))], 150)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(200).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let edge = snapshot.get_edge(0, make_edge_key(1, 0));
        assert!(edge.is_some());
        assert_eq!(edge.unwrap().edge_id, EdgeId(0));
        assert!(snapshot.get_edge(0, make_edge_key(99, 0)).is_none());

        assert_eq!(snapshot.degree(5), 1);
        assert_eq!(snapshot.degree(0), 1);
    }

    #[test]
    fn test_cold_snapshot_integrity() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        ColdSnapshot::create(&exported, &path).unwrap();

        let mut corrupted = std::fs::read(&path).unwrap();
        if let Some(b) = corrupted.last_mut() {
            *b ^= 0xFF;
        }
        std::fs::write(&path, &corrupted).unwrap();

        let result = ColdSnapshot::open(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_cold_snapshot_vertex_capacity() {
        let mut table = make_table();
        for i in 0..10u32 {
            table
                .insert_edge(
                    i,
                    i + 1,
                    0,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    100,
                )
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert!(snapshot.vertex_capacity() >= 10);
    }

    #[test]
    fn test_cold_snapshot_scan_edges() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        table
            .insert_edge(3, 4, 0, &[("weight".to_string(), Value::Double(3.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let scanned = snapshot.scan_edges();
        assert_eq!(scanned.len(), 3);

        let keys: Vec<(u32, VertexId)> = scanned
            .iter()
            .map(|r| (r.src_internal, r.dst_vid))
            .collect();
        assert!(keys.contains(&(0, VertexId::from_int64(1))));
        assert!(keys.contains(&(0, VertexId::from_int64(2))));
        assert!(keys.contains(&(3, VertexId::from_int64(4))));
    }

    #[test]
    fn test_cold_snapshot_get_edge_to_dst() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 7, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 3, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let nbr = snapshot.get_edge_to_dst(0, 1).expect("edge 0->1 exists");
        let decoded_dst = VertexId::from_int64(nbr.endpoint as i64);
        let decoded_rank = nbr.rank;
        assert_eq!(decoded_dst, VertexId::from_int64(1));
        assert_eq!(decoded_rank, 7);
        assert!(snapshot.get_edge_to_dst(0, 99).is_none());
        assert!(snapshot.get_edge_to_dst(1, 1).is_none());
    }

    #[test]
    fn test_cold_snapshot_nbr_to_edge_record_rank() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 42, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let nbr = snapshot.get_edge_to_dst(0, 1).expect("edge exists");
        let record =
            snapshot.nbr_to_edge_record(&nbr, VertexId::from_int64(0), VertexId::from_int64(1));
        assert_eq!(record.rank, 42);
    }

    #[test]
    fn test_cold_snapshot_nbr_to_edge_record() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(42.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        // Query edge and convert to record
        if let Some(nbr) = snapshot.get_edge(0, make_edge_key(1, 0)) {
            let src_vid = VertexId::from_int64(0);
            let dst_vid = make_edge_key(1, 0);
            let record = snapshot.nbr_to_edge_record(&nbr, src_vid, dst_vid);

            assert_eq!(record.src_vid, src_vid);
            assert_eq!(record.dst_vid, dst_vid);
            assert!(!record.properties.is_empty());
            assert_eq!(record.properties[0].0, "weight");
            assert_eq!(record.properties[0].1, Value::Double(42.0));
        } else {
            panic!("expected edge to exist");
        }
    }

    #[test]
    fn test_cold_snapshot_scan_edges_empty() {
        let table = make_table();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let scanned = snapshot.scan_edges();
        assert!(scanned.is_empty());
    }

    #[test]
    fn test_cold_snapshot_remap_vertex_ids() {
        let mut table = make_table();
        for (src, dst) in [(0u32, 4u32), (2, 5), (4, 5), (5, 4)] {
            table
                .insert_edge(
                    src,
                    dst,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let mut snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert_eq!(snapshot.vertex_capacity(), 6);
        assert_eq!(snapshot.edge_count(), 4);

        let mapping: HashMap<u32, u32> = [(4, 3), (5, 4)].into_iter().collect();
        snapshot
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();

        assert_eq!(snapshot.vertex_capacity(), 5);
        assert_eq!(snapshot.edge_count(), 4);
        assert_eq!(snapshot.get_out_edges(3).len(), 1);
        assert_eq!(snapshot.get_out_edges(4).len(), 1);
        assert_eq!(snapshot.get_out_edges(5).len(), 0);
        assert_eq!(snapshot.get_in_edges(3).len(), 2);
        assert_eq!(snapshot.get_in_edges(4).len(), 2);
        assert_eq!(snapshot.degree(3), 1);
    }

    #[test]
    fn test_cold_snapshot_remap_persists_to_file() {
        let mut table = make_table();
        for (src, dst) in [(0u32, 4u32), (2, 5), (4, 5), (5, 4)] {
            table
                .insert_edge(
                    src,
                    dst,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let mut snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert_eq!(snapshot.vertex_capacity(), 6);

        let mapping: HashMap<u32, u32> = [(4, 3), (5, 4)].into_iter().collect();
        snapshot
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();
        snapshot.persist().unwrap();

        // A reload from the rewritten file must observe the remap.
        let reloaded = ColdSnapshot::open(&path).unwrap();
        assert_eq!(reloaded.vertex_capacity(), 5);
        assert_eq!(reloaded.edge_count(), 4);
        assert_eq!(reloaded.get_out_edges(3).len(), 1);
        assert_eq!(reloaded.get_out_edges(4).len(), 1);
        assert_eq!(reloaded.get_out_edges(5).len(), 0);
        assert_eq!(reloaded.get_in_edges(3).len(), 2);
        assert_eq!(reloaded.get_in_edges(4).len(), 2);
        assert_eq!(reloaded.degree(3), 1);
    }

    #[test]
    fn test_cold_snapshot_remap_dst_only() {
        let mut table = make_table();
        for (src, dst) in [(0u32, 4u32), (2, 5), (4, 5), (5, 4)] {
            table
                .insert_edge(
                    src,
                    dst,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let mut snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let mapping: HashMap<u32, u32> = [(4, 3), (5, 4)].into_iter().collect();
        snapshot.remap_vertex_ids(None, Some(&mapping)).unwrap();

        assert_eq!(snapshot.vertex_capacity(), 6);
        assert_eq!(snapshot.edge_count(), 4);
        assert_eq!(snapshot.get_out_edges(4).len(), 1);
        assert_eq!(snapshot.get_out_edges(5).len(), 1);
        assert_eq!(snapshot.get_in_edges(3).len(), 2);
        assert_eq!(snapshot.get_in_edges(4).len(), 2);
        assert_eq!(snapshot.get_in_edges(5).len(), 0);
    }

    #[test]
    fn test_cold_snapshot_property_index_build_and_lookup() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
            .unwrap();
        table
            .insert_edge(3, 4, 0, &[("weight".to_string(), Value::Double(3.5))], 100)
            .unwrap();

        let exported = table.export_snapshot(100).unwrap();
        let index = ColdPropertyIndex::build(&exported, &["weight".to_string()]);
        assert!(!index.is_empty());
        assert_eq!(index.indexed_property_names(), vec!["weight".to_string()]);

        let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
        // Equality lookup: weight = 2.5
        let key = codec.encode(&Value::Double(2.5)).unwrap();
        let upper = graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&key);
        let hits = index.lookup("weight", &key, &upper);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].src_internal, 0);
        assert_eq!(hits[0].dst_internal, 2);
        assert_eq!(hits[0].rank, 0);

        // Range lookup: weight >= 2.5
        let hits = index.lookup("weight", &key, &[0xFF]);
        assert_eq!(hits.len(), 2);

        // Unknown property yields nothing
        assert!(index.lookup("missing", &key, &[0xFF]).is_empty());
    }

    #[test]
    fn test_cold_snapshot_property_index_roundtrip() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 5, &[("weight".to_string(), Value::Double(9.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let index = ColdPropertyIndex::build(&exported, &["weight".to_string()]);
        let snapshot = ColdSnapshot::create_with_index(&exported, Some(index), &path).unwrap();
        assert!(snapshot.has_property_index());

        let loaded = ColdSnapshot::open(&path).unwrap();
        assert!(loaded.has_property_index());
        let index = loaded.property_index().unwrap();
        let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
        let key = codec.encode(&Value::Double(9.0)).unwrap();
        let hits = index.lookup(
            "weight",
            &key,
            &graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&key),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].src_internal, 0);
        assert_eq!(hits[0].dst_internal, 1);
        assert_eq!(hits[0].rank, 5);
    }

    #[test]
    fn test_cold_snapshot_property_index_remap() {
        let mut table = make_table();
        table
            .insert_edge(0, 5, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let index = ColdPropertyIndex::build(&exported, &["weight".to_string()]);
        let mut snapshot = ColdSnapshot::create_with_index(&exported, Some(index), &path).unwrap();

        let mapping: HashMap<u32, u32> = [(5, 3)].into_iter().collect();
        snapshot
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();
        snapshot.persist().unwrap();

        let reloaded = ColdSnapshot::open(&path).unwrap();
        let index = reloaded.property_index().unwrap();
        let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
        let key = codec.encode(&Value::Double(1.0)).unwrap();
        let hits = index.lookup(
            "weight",
            &key,
            &graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&key),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].src_internal, 0);
        assert_eq!(hits[0].dst_internal, 3);
    }

    #[test]
    fn test_property_table_read_properties() {
        use crate::edge::PropertyTable;

        let mut pt = PropertyTable::new();
        pt.add_property("name".to_string(), graphdb_core::DataType::String, false)
            .unwrap();
        pt.add_property("age".to_string(), graphdb_core::DataType::Int, false)
            .unwrap();

        let offset = pt
            .insert(
                &[
                    ("name".to_string(), Value::String("Alice".into())),
                    ("age".to_string(), Value::Int(30)),
                ],
                100,
            )
            .unwrap();

        let props = pt.read_properties(offset).unwrap();
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].0, "name");
        assert_eq!(props[0].1, Value::String("Alice".into()));
        assert_eq!(props[1].0, "age");
        assert_eq!(props[1].1, Value::Int(30));

        // Missing offset returns None
        assert!(pt.read_properties(u32::MAX).is_none());
    }

    #[test]
    fn test_cold_snapshot_presence_bitmap() {
        let mut table = make_table();
        for src in [0u32, 5, 100] {
            table
                .insert_edge(
                    src,
                    src + 1,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let presence = snapshot.vertex_presence().expect("presence bitmap present");
        assert!(snapshot.row_has_edges(0));
        assert!(snapshot.row_has_edges(5));
        assert!(snapshot.row_has_edges(100));
        assert!(!snapshot.row_has_edges(1));
        assert!(!snapshot.row_has_edges(200));

        let loaded = ColdSnapshot::open(&path).unwrap();
        assert_eq!(loaded.vertex_presence(), Some(presence));
        assert_eq!(loaded.scan_edges().len(), 3);
        assert!(loaded.row_has_edges(5));
        assert!(!loaded.row_has_edges(6));
    }

    #[test]
    fn test_cold_snapshot_dict_encoding_smaller() {
        // 100 sources x 5 destinations = 500 edges over 6 distinct endpoints:
        // dict encoding (24 B/edge + tiny dict) beats raw (37 B/edge).
        let mut table = make_table();
        for src in 0..100u32 {
            for dst in 0..5u32 {
                table
                    .insert_edge(
                        src,
                        dst,
                        0,
                        &[("weight".to_string(), Value::Double(1.0))],
                        100,
                    )
                    .unwrap();
            }
        }
        let csr = table.export_snapshot(100).unwrap().out_csr;
        let raw = csr.dump();
        let dict = encode_csr_dict(&csr);
        assert!(
            dict.len() < raw.len(),
            "dict encoding should shrink repetitive CSRs: dict={}, raw={}",
            dict.len(),
            raw.len()
        );

        let roundtrip = decode_csr_dict(&dict).unwrap();
        assert_eq!(roundtrip.edge_count(), csr.edge_count());
        assert_eq!(roundtrip.edges_of(0).len(), 5);
        assert_eq!(roundtrip.edges_of(99).len(), 5);
        assert_eq!(roundtrip.edges_of(50).len(), 5);
        // Endpoint bytes survive the dict round-trip.
        assert_eq!(
            roundtrip.edges_of(0)[0].to_vertex_id(),
            csr.edges_of(0)[0].to_vertex_id()
        );
    }

    #[test]
    fn test_cold_snapshot_zstd_property_section() {
        // Large string-heavy property payloads must compress.
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "name".to_string(),
                graphdb_core::types::DataType::String,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();
        for i in 0..50u32 {
            table
                .insert_edge(
                    i,
                    i + 1,
                    0,
                    &[(
                        "name".to_string(),
                        Value::string(format!("repeated-pattern-{}", i % 7)),
                    )],
                    100,
                )
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        ColdSnapshot::create(&exported, &path).unwrap();
        let loaded = ColdSnapshot::open(&path).unwrap();
        assert_eq!(loaded.edge_count(), 50);
        let nbr = loaded.get_out_edges(10)[0];
        let props = loaded
            .properties()
            .read_properties_by_edge_id(nbr.edge_id)
            .unwrap();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].1, Value::string("repeated-pattern-3"));
    }
}
