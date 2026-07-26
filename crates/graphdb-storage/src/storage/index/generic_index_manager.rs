//! Generic Index Manager
//!
//! This module provides a generic implementation of index management
//! that can be used for both vertex and edge indexes.

use crate::core::types::storage_ids::VertexId;
use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult};
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::IndexKeyGenerator;
use crate::storage::index::types::IndexRecord;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::{
    atomic::AtomicU64,
    Arc,
};

type LoadedIndexData = (
    BTreeMap<SecondaryIndexKey, IndexRecord>,
    BTreeMap<SecondaryIndexKey, IndexRecord>,
    u64,
);

/// Generic index manager
///
/// Provides common functionality for index management including:
/// - In-memory storage with BTreeMap
/// - Persistence (flush/load)
/// - GC for tombstones
pub struct GenericIndexManager<K: IndexKeyGenerator> {
    forward_index: Arc<RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>>>,
    reverse_index: Arc<RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>>>,
    version_counter: Arc<AtomicU64>,
    _marker: PhantomData<K>,
}

impl<K: IndexKeyGenerator> Clone for GenericIndexManager<K> {
    fn clone(&self) -> Self {
        Self {
            forward_index: Arc::clone(&self.forward_index),
            reverse_index: Arc::clone(&self.reverse_index),
            version_counter: Arc::clone(&self.version_counter),
            _marker: PhantomData,
        }
    }
}

impl<K: IndexKeyGenerator> GenericIndexManager<K> {
    pub fn new() -> Self {
        Self {
            forward_index: Arc::new(RwLock::new(BTreeMap::new())),
            reverse_index: Arc::new(RwLock::new(BTreeMap::new())),
            version_counter: Arc::new(AtomicU64::new(1)),
            _marker: PhantomData,
        }
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> (usize, usize) {
        (
            self.forward_index.read().len(),
            self.reverse_index.read().len(),
        )
    }

    pub(crate) fn flush_data<P: AsRef<Path>>(
        path: P,
        forward: &BTreeMap<SecondaryIndexKey, IndexRecord>,
        reverse: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) -> StorageResult<()> {
        let path = path.as_ref();
        std::fs::create_dir_all(path)?;
        Self::flush_index_file(&path.join("forward_index.bin"), forward)?;
        Self::flush_index_file(&path.join("reverse_index.bin"), reverse)?;
        std::fs::File::open(path)?.sync_all()?;
        Ok(())
    }

    fn flush_index_map<W: std::io::Write>(
        writer: &mut W,
        index: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) -> std::io::Result<()> {
        let count = index.len() as u64;
        writer.write_all(&count.to_le_bytes())?;

        for (key, entry) in index.iter() {
            writer.write_all(&(key.len() as u32).to_le_bytes())?;
            writer.write_all(key)?;
            writer.write_all(&entry.created_ts.to_le_bytes())?;
            if let Some(deleted_ts) = entry.deleted_ts {
                writer.write_all(&[1u8])?;
                writer.write_all(&deleted_ts.to_le_bytes())?;
            } else {
                writer.write_all(&[0u8])?;
            }

            let num_included = entry.included_columns.len() as u32;
            writer.write_all(&num_included.to_le_bytes())?;
            for (name, value) in &entry.included_columns {
                let name_bytes = name.as_bytes();
                writer.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
                writer.write_all(name_bytes)?;
                let value_bytes = OrderedCodec::new().encode(value).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                })?;
                writer.write_all(&(value_bytes.len() as u32).to_le_bytes())?;
                writer.write_all(&value_bytes)?;
            }

            write_entity_ref(writer, &entry.entity_ref)?;

            if let Some(encoded) = &entry.encoded_indexed_value {
                writer.write_all(&[1u8])?;
                writer.write_all(&(encoded.len() as u32).to_le_bytes())?;
                writer.write_all(encoded)?;
            } else {
                writer.write_all(&[0u8])?;
            }

            if let Some(entity_version) = entry.entity_version {
                writer.write_all(&[1u8])?;
                writer.write_all(&entity_version.to_le_bytes())?;
            } else {
                writer.write_all(&[0u8])?;
            }
        }

        Ok(())
    }

    fn flush_index_file(
        path: &Path,
        index: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) -> StorageResult<()> {
        let temporary = path.with_extension("tmp");
        let mut file = std::fs::File::create(&temporary)?;
        Self::flush_index_map(&mut file, index)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    /// Atomically replace the inner forward and reverse index data.
    /// This enables crash-safe generation rebuilds: build a new BTreeMap in isolation,
    /// flush it to a checkpoint file, then swap in one write-lock cycle.
    pub fn replace_data(
        &self,
        forward: BTreeMap<SecondaryIndexKey, IndexRecord>,
        reverse: BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) {
        *self.forward_index.write() = forward;
        *self.reverse_index.write() = reverse;
    }

    pub(crate) fn load_data<P: AsRef<Path>>(path: P) -> StorageResult<LoadedIndexData> {
        let path = path.as_ref();
        let loader = Self::new();
        let (forward_index, forward_max_version) =
            loader.load_index_file(&path.join("forward_index.bin"))?;
        let (reverse_index, reverse_max_version) =
            loader.load_index_file(&path.join("reverse_index.bin"))?;
        Ok((
            forward_index,
            reverse_index,
            forward_max_version
                .max(reverse_max_version)
                .saturating_add(1),
        ))
    }

    fn load_index_file(
        &self,
        path: &Path,
    ) -> StorageResult<(BTreeMap<SecondaryIndexKey, IndexRecord>, u64)> {
        use std::fs::File;
        use std::io::Read;

        if !path.exists() {
            return Ok((BTreeMap::new(), 0));
        }

        let mut file = File::open(path)?;

        let mut count_bytes = [0u8; 8];
        file.read_exact(&mut count_bytes)?;
        let count = u64::from_le_bytes(count_bytes);

        let mut index = BTreeMap::new();
        let mut max_version = 0u64;

        for _ in 0..count {
            let mut key_len_bytes = [0u8; 4];
            file.read_exact(&mut key_len_bytes)?;
            let key_len = u32::from_le_bytes(key_len_bytes) as usize;

            let mut key = vec![0u8; key_len];
            file.read_exact(&mut key)?;

            let mut created_ts_bytes = [0u8; 4];
            file.read_exact(&mut created_ts_bytes)?;
            let created_ts = u32::from_le_bytes(created_ts_bytes);

            let mut has_deleted = [0u8; 1];
            file.read_exact(&mut has_deleted)?;
            let deleted_ts = if has_deleted[0] == 1 {
                let mut deleted_ts_bytes = [0u8; 4];
                file.read_exact(&mut deleted_ts_bytes)?;
                Some(u32::from_le_bytes(deleted_ts_bytes))
            } else {
                None
            };

            let mut included_columns = Vec::new();
            let mut num_included_bytes = [0u8; 4];
            if file.read_exact(&mut num_included_bytes).is_ok() {
                let num_included = u32::from_le_bytes(num_included_bytes) as usize;
                for _ in 0..num_included {
                    let mut name_len_bytes = [0u8; 4];
                    file.read_exact(&mut name_len_bytes)?;
                    let name_len = u32::from_le_bytes(name_len_bytes) as usize;
                    let mut name_bytes = vec![0u8; name_len];
                    file.read_exact(&mut name_bytes)?;
                    let name = String::from_utf8(name_bytes).map_err(|e| {
                        StorageError::db_error(format!("Invalid included column name: {e}"))
                    })?;

                    let mut value_len_bytes = [0u8; 4];
                    file.read_exact(&mut value_len_bytes)?;
                    let value_len = u32::from_le_bytes(value_len_bytes) as usize;
                    let mut value_bytes = vec![0u8; value_len];
                    file.read_exact(&mut value_bytes)?;
                    let value = OrderedCodec::new().decode(&value_bytes)?;
                    included_columns.push((name, value));
                }
            }

            let entity_ref = read_entity_ref(&mut file)?;

            let mut has_encoded_value = [0u8; 1];
            let encoded_indexed_value = if file.read_exact(&mut has_encoded_value).is_ok() && has_encoded_value[0] == 1 {
                let mut encoded_len_bytes = [0u8; 4];
                file.read_exact(&mut encoded_len_bytes)?;
                let encoded_len = u32::from_le_bytes(encoded_len_bytes) as usize;
                let mut encoded = vec![0u8; encoded_len];
                file.read_exact(&mut encoded)?;
                Some(encoded)
            } else {
                None
            };

            let mut has_entity_version = [0u8; 1];
            if file.read_exact(&mut has_entity_version).is_ok() {
                let entity_version = if has_entity_version[0] == 1 {
                    let mut bytes = [0u8; 4];
                    file.read_exact(&mut bytes)?;
                    Some(u32::from_le_bytes(bytes))
                } else {
                    None
                };
                let entry = IndexRecord {
                    created_ts,
                    deleted_ts,
                    entity_version,
                    included_columns,
                    entity_ref,
                    encoded_indexed_value: encoded_indexed_value.clone(),
                };
                max_version = max_version.max(Self::extract_version_from_key(&key));
                index.insert(key, entry);
            } else {
                let entry = IndexRecord {
                    created_ts,
                    deleted_ts,
                    entity_version: None,
                    included_columns,
                    entity_ref,
                    encoded_indexed_value: encoded_indexed_value.clone(),
                };
                max_version = max_version.max(Self::extract_version_from_key(&key));
                index.insert(key, entry);
            }
        }

        Ok((index, max_version))
    }

    pub(crate) fn forward_index_handle(
        &self,
    ) -> Arc<RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>>> {
        Arc::new(RwLock::new(self.forward_index.read().clone()))
    }

    fn extract_version_from_key(key: &[u8]) -> u64 {
        if key.len() < std::mem::size_of::<u64>() {
            return 0;
        }

        let start = key.len() - std::mem::size_of::<u64>();
        let mut bytes = [0u8; std::mem::size_of::<u64>()];
        bytes.copy_from_slice(&key[start..]);
        u64::from_le_bytes(bytes)
    }

}

impl<K: IndexKeyGenerator> Default for GenericIndexManager<K> {
    fn default() -> Self {
        Self::new()
    }
}

// ── EntityRef binary serialization helpers ──

fn write_entity_ref<W: std::io::Write>(
    writer: &mut W,
    entity_ref: &Option<EntityRef>,
) -> std::io::Result<()> {
    match entity_ref {
        None => writer.write_all(&[0u8]),
        Some(EntityRef::Vertex(vid)) => {
            writer.write_all(&[1u8])?;
            let bytes = vid.as_bytes();
            let len = bytes.len().min(u8::MAX as usize) as u8;
            writer.write_all(&[len])?;
            writer.write_all(&bytes[..len as usize])
        }
        Some(EntityRef::Edge {
            src,
            dst,
            edge_type,
            ranking,
        }) => {
            writer.write_all(&[2u8])?;
            let src_bytes = src.as_bytes();
            let src_len = src_bytes.len().min(u8::MAX as usize) as u8;
            writer.write_all(&[src_len])?;
            writer.write_all(&src_bytes[..src_len as usize])?;
            let dst_bytes = dst.as_bytes();
            let dst_len = dst_bytes.len().min(u8::MAX as usize) as u8;
            writer.write_all(&[dst_len])?;
            writer.write_all(&dst_bytes[..dst_len as usize])?;
            writer.write_all(&edge_type.to_le_bytes())?;
            writer.write_all(&ranking.to_le_bytes())
        }
    }
}

fn read_entity_ref<R: std::io::Read>(reader: &mut R) -> std::io::Result<Option<EntityRef>> {
    let mut tag = [0u8; 1];
    reader.read_exact(&mut tag)?;
    match tag[0] {
        0 => Ok(None),
        1 => {
            let mut len = [0u8; 1];
            reader.read_exact(&mut len)?;
            let mut bytes = vec![0u8; len[0] as usize];
            reader.read_exact(&mut bytes)?;
            let vid = VertexId::from_bytes(bytes);
            Ok(Some(EntityRef::Vertex(vid)))
        }
        2 => {
            let mut len = [0u8; 1];
            reader.read_exact(&mut len)?;
            let mut src_bytes = vec![0u8; len[0] as usize];
            reader.read_exact(&mut src_bytes)?;
            let src = VertexId::from_bytes(src_bytes);

            reader.read_exact(&mut len)?;
            let mut dst_bytes = vec![0u8; len[0] as usize];
            reader.read_exact(&mut dst_bytes)?;
            let dst = VertexId::from_bytes(dst_bytes);

            let mut edge_type_bytes = [0u8; 4];
            reader.read_exact(&mut edge_type_bytes)?;
            let edge_type = u32::from_le_bytes(edge_type_bytes);

            let mut ranking_bytes = [0u8; 8];
            reader.read_exact(&mut ranking_bytes)?;
            let ranking = i64::from_le_bytes(ranking_bytes);

            Ok(Some(EntityRef::Edge {
                src,
                dst,
                edge_type,
                ranking,
            }))
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Unknown EntityRef tag",
        )),
    }
}
