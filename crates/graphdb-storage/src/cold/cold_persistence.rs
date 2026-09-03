use std::collections::HashMap;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::cold::cold_property_index::ColdPropertyIndex;
use crate::cold::cold_snapshot::ColdSnapshot;
use crate::edge::edge_table::snapshot::ExportedEdgeSnapshot;
use crate::edge::{Csr, CsrBase, CsrWithProperties, EdgeSchema};
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

pub const COLD_SNAPSHOT_MAGIC: [u8; 4] = *b"LKCS";
/// Current on-disk format version.
pub const COLD_SNAPSHOT_VERSION: u32 = 1;
pub(crate) const HEADER_SIZE: usize = 36;

const ZSTD_MARKER: u8 = 0x01;
const RAW_MARKER: u8 = 0x00;
const ZSTD_LEVEL: i32 = 3;

const CSR_RAW: u8 = 0x00;
const CSR_DICT: u8 = 0x01;

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

        let schema_json = std::str::from_utf8(schema_data)
            .map_err(|e| StorageError::deserialize_error(format!("invalid schema utf-8: {}", e)))?;
        let schema: EdgeSchema = serde_json::from_str(schema_json)
            .map_err(|e| StorageError::deserialize_error(format!("invalid schema json: {}", e)))?;

        // Build property schemas from EdgeSchema so columns match during load
        let prop_schemas: Vec<crate::edge::property_schema::PropertySchema> = schema
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| {
                crate::edge::property_schema::PropertySchema::new(
                    p.name.clone(),
                    i as i32,
                    p.data_type.clone(),
                )
                .nullable(p.nullable)
            })
            .collect();
        let mut properties = CsrWithProperties::new(vertex_capacity.max(1), prop_schemas);
        properties.load(&prop_data)?;

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
            properties: CsrWithProperties::new(1, Vec::new()),
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
        properties: CsrWithProperties,
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

    /// Backing `.lkcs` file path when the snapshot was opened from or
    /// created at a path.
    pub(crate) fn backing_path(&self) -> Option<&Path> {
        self.path.as_deref()
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
#[allow(clippy::too_many_arguments)]
fn encode_snapshot(
    out_csr: &Csr,
    in_csr: &Csr,
    properties: &CsrWithProperties,
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

    // Edge property map section: write empty section for backward compat.
    // New files no longer need this section since properties are in CsrWithProperties.
    write_section(&mut buf, &[]);

    let checksum = crc32fast::hash(&buf);
    buf.extend_from_slice(&checksum.to_le_bytes());

    Ok(buf)
}

/// Build the out-row presence bitmap: bit v set = vertex row v has at least
/// one out edge. Rows past the CSR capacity are implicitly absent.
pub(crate) fn build_presence_bitmap(csr: &Csr) -> Vec<u64> {
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

/// Dict-encode a CSR (payload of a `CSR_DICT` section).
pub(crate) fn encode_csr_dict(csr: &Csr) -> Vec<u8> {
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
pub(crate) fn decode_csr_dict(data: &[u8]) -> StorageResult<Csr> {
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
    let mut entries: Vec<(u32, crate::edge::Nbr, Timestamp)> = Vec::with_capacity(edge_count);
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
                crate::edge::Nbr::new(endpoint_vid.as_int64().unwrap_or(0) as u32, rank, edge_id),
                ts,
            ));
        }
    }
    Ok(Csr::from_nbr_entries(&entries, capacity))
}
