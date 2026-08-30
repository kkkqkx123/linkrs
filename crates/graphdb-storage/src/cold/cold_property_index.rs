use std::collections::HashMap;

use crate::edge::edge_table::snapshot::ExportedEdgeSnapshot;
use graphdb_core::{StorageError, StorageResult};

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
                let Some(props) = exported.properties.read_properties(nbr.prop_offset) else {
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

    pub(crate) fn encode(&self) -> Vec<u8> {
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

    pub(crate) fn decode(data: &[u8]) -> StorageResult<Self> {
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
