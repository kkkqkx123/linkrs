//! Generic Index Manager
//!
//! This module provides a generic implementation of index management
//! that can be used for both vertex and edge indexes.

use crate::core::types::storage_ids::VertexId;
use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult};
use crate::storage::index::art::ArtTree;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::IndexKeyGenerator;
use crate::storage::index::types::IndexRecord;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::io::Write;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

const MAGIC_FORWARD: [u8; 4] = *b"INDF";
const MAGIC_REVERSE: [u8; 4] = *b"INDR";

type LoadedIndexData = (
    BTreeMap<SecondaryIndexKey, IndexRecord>,
    BTreeMap<SecondaryIndexKey, IndexRecord>,
);

/// Generic index manager
///
/// Provides common functionality for index management including:
/// - In-memory storage with ART tree (replacing BTreeMap for better memory efficiency)
/// - Persistence (flush/load via BTreeMap serialization)
/// - GC for tombstones
pub struct GenericIndexManager<K: IndexKeyGenerator> {
    forward_index: Arc<RwLock<ArtTree<IndexRecord>>>,
    reverse_index: Arc<RwLock<ArtTree<IndexRecord>>>,
    _marker: PhantomData<K>,
}

impl<K: IndexKeyGenerator> Clone for GenericIndexManager<K> {
    fn clone(&self) -> Self {
        Self {
            forward_index: Arc::clone(&self.forward_index),
            reverse_index: Arc::clone(&self.reverse_index),
            _marker: PhantomData,
        }
    }
}

impl<K: IndexKeyGenerator> GenericIndexManager<K> {
    pub fn new() -> Self {
        Self {
            forward_index: Arc::new(RwLock::new(ArtTree::new())),
            reverse_index: Arc::new(RwLock::new(ArtTree::new())),
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

    /// Access the underlying ART tree (read-only).
    #[cfg(test)]
    pub(crate) fn read_forward(&self) -> parking_lot::RwLockReadGuard<'_, ArtTree<IndexRecord>> {
        self.forward_index.read()
    }

    /// Access the underlying ART tree (read-only).
    #[cfg(test)]
    pub(crate) fn read_reverse(&self) -> parking_lot::RwLockReadGuard<'_, ArtTree<IndexRecord>> {
        self.reverse_index.read()
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
            let mut hasher = crc32fast::Hasher::new();
            let key_len = key.len() as u32;
            hasher.update(&key_len.to_le_bytes());
            hasher.update(key);
            hasher.update(&entry.created_ts.to_le_bytes());
            if let Some(deleted_ts) = entry.deleted_ts {
                hasher.update(&[1u8]);
                hasher.update(&deleted_ts.to_le_bytes());
            } else {
                hasher.update(&[0u8]);
            }
            let num_included = entry.included_columns.as_ref().map_or(0, |v| v.len()) as u32;
            hasher.update(&num_included.to_le_bytes());
            if let Some(columns) = &entry.included_columns {
                for (name, value) in columns {
                    let name_bytes = name.as_bytes();
                    hasher.update(&(name_bytes.len() as u32).to_le_bytes());
                    hasher.update(name_bytes);
                    let value_bytes = OrderedCodec::new().encode(value).map_err(|e| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                    })?;
                    hasher.update(&(value_bytes.len() as u32).to_le_bytes());
                    hasher.update(&value_bytes);
                }
            }

            let entity_ref_encoded = encode_entity_ref(&entry.entity_ref);
            hasher.update(&entity_ref_encoded);

            if let Some(entity_version) = entry.entity_version {
                hasher.update(&[1u8]);
                hasher.update(&entity_version.to_le_bytes());
            } else {
                hasher.update(&[0u8]);
            }

            writer.write_all(&key_len.to_le_bytes())?;
            writer.write_all(key)?;
            writer.write_all(&entry.created_ts.to_le_bytes())?;
            if let Some(deleted_ts) = entry.deleted_ts {
                writer.write_all(&[1u8])?;
                writer.write_all(&deleted_ts.to_le_bytes())?;
            } else {
                writer.write_all(&[0u8])?;
            }
            writer.write_all(&num_included.to_le_bytes())?;
            if let Some(columns) = &entry.included_columns {
                for (name, value) in columns {
                    let name_bytes = name.as_bytes();
                    writer.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
                    writer.write_all(name_bytes)?;
                    let value_bytes = OrderedCodec::new().encode(value).map_err(|e| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                    })?;
                    writer.write_all(&(value_bytes.len() as u32).to_le_bytes())?;
                    writer.write_all(&value_bytes)?;
                }
            }
            write_entity_ref(writer, &entry.entity_ref)?;
            if let Some(entity_version) = entry.entity_version {
                writer.write_all(&[1u8])?;
                writer.write_all(&entity_version.to_le_bytes())?;
            } else {
                writer.write_all(&[0u8])?;
            }

            let checksum = hasher.finalize();
            writer.write_all(&checksum.to_le_bytes())?;
        }

        Ok(())
    }

    fn flush_index_file(
        path: &Path,
        index: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) -> StorageResult<()> {
        let temporary = path.with_extension("tmp");
        let mut file = std::fs::File::create(&temporary)?;
        let magic = if path.to_string_lossy().contains("forward") {
            MAGIC_FORWARD
        } else {
            MAGIC_REVERSE
        };
        file.write_all(&magic)?;
        Self::flush_index_map(&mut file, index)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    pub(crate) fn load_data<P: AsRef<Path>>(path: P) -> StorageResult<LoadedIndexData> {
        let path = path.as_ref();
        let loader = Self::new();
        let forward_index = loader.load_index_file(&path.join("forward_index.bin"))?;
        let reverse_index = loader.load_index_file(&path.join("reverse_index.bin"))?;
        Ok((forward_index, reverse_index))
    }

    fn load_index_file(
        &self,
        path: &Path,
    ) -> StorageResult<BTreeMap<SecondaryIndexKey, IndexRecord>> {
        use std::fs::File;
        use std::io::Read;

        if !path.exists() {
            return Ok(BTreeMap::new());
        }

        let mut file = File::open(path)?;
        let file_size = file.metadata()?.len();
        if file_size < 12 {
            return Err(StorageError::db_error(format!(
                "Index file too small ({file_size} bytes), likely corrupted: {path:?}"
            )));
        }

        let expected_magic = if path.to_string_lossy().contains("forward") {
            MAGIC_FORWARD
        } else {
            MAGIC_REVERSE
        };
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        if magic != expected_magic {
            return Err(StorageError::db_error(format!(
                "Corrupted index file: magic mismatch in {:?}",
                path
            )));
        }

        let mut count_bytes = [0u8; 8];
        file.read_exact(&mut count_bytes)?;
        let count = u64::from_le_bytes(count_bytes);

        let mut index = BTreeMap::new();
        let mut loaded = 0u64;

        for _ in 0..count {
            let mut key_len_bytes = [0u8; 4];
            file.read_exact(&mut key_len_bytes)?;
            let key_len = u32::from_le_bytes(key_len_bytes) as usize;

            let mut key = vec![0u8; key_len];
            file.read_exact(&mut key)?;

            let mut created_ts_bytes = [0u8; 8];
            file.read_exact(&mut created_ts_bytes)?;
            let created_ts = u64::from_le_bytes(created_ts_bytes);

            let mut has_deleted = [0u8; 1];
            file.read_exact(&mut has_deleted)?;
            let deleted_ts = if has_deleted[0] == 1 {
                let mut deleted_ts_bytes = [0u8; 8];
                file.read_exact(&mut deleted_ts_bytes)?;
                Some(u64::from_le_bytes(deleted_ts_bytes))
            } else {
                None
            };

            let mut num_included_bytes = [0u8; 4];
            file.read_exact(&mut num_included_bytes)?;
            let num_included = u32::from_le_bytes(num_included_bytes) as usize;
            let mut included_columns = Vec::with_capacity(num_included);
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

            let entity_ref = read_entity_ref(&mut file)?;

            let mut has_entity_version = [0u8; 1];
            file.read_exact(&mut has_entity_version)?;
            let entity_version = if has_entity_version[0] == 1 {
                let mut bytes = [0u8; 8];
                file.read_exact(&mut bytes)?;
                Some(u64::from_le_bytes(bytes))
            } else {
                None
            };

            let mut stored_checksum = [0u8; 4];
            file.read_exact(&mut stored_checksum)?;
            let stored_checksum = u32::from_le_bytes(stored_checksum);

            // Verify checksum
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&key_len_bytes);
            hasher.update(&key);
            hasher.update(&created_ts_bytes);
            hasher.update(&has_deleted);
            if let Some(dts) = deleted_ts {
                hasher.update(&dts.to_le_bytes());
            }
            hasher.update(&num_included_bytes);
            for (name, _) in &included_columns {
                hasher.update(&(name.len() as u32).to_le_bytes());
                hasher.update(name.as_bytes());
            }
            for (_, value) in &included_columns {
                let encoded = OrderedCodec::new().encode(value).map_err(|e| {
                    StorageError::db_error(format!("Re-encode error for checksum: {e}"))
                })?;
                hasher.update(&(encoded.len() as u32).to_le_bytes());
                hasher.update(&encoded);
            }
            hasher.update(&encode_entity_ref(&entity_ref));
            hasher.update(&has_entity_version);
            if let Some(ev) = entity_version {
                hasher.update(&ev.to_le_bytes());
            }
            let computed = hasher.finalize();
            if computed != stored_checksum {
                return Err(StorageError::db_error(format!(
                    "Checksum mismatch in index file {:?}",
                    path
                )));
            }

            let entry = IndexRecord {
                created_ts,
                deleted_ts,
                entity_version,
                included_columns: Some(included_columns),
                entity_ref,
            };
            index.insert(key, entry);
            loaded += 1;
        }

        let mut trailing = Vec::new();
        file.read_to_end(&mut trailing)?;
        if !trailing.is_empty() {
            return Err(StorageError::db_error(format!(
                "Index file has {} trailing bytes after {loaded} entries: {path:?}",
                trailing.len()
            )));
        }

        Ok(index)
    }
}

impl<K: IndexKeyGenerator> Default for GenericIndexManager<K> {
    fn default() -> Self {
        Self::new()
    }
}

// ── EntityRef binary serialization helpers ──

pub(crate) fn encode_entity_ref(entity_ref: &Option<EntityRef>) -> Vec<u8> {
    let mut buf = Vec::new();
    write_entity_ref(&mut buf, entity_ref).expect("Vec::write is infallible");
    buf
}

pub(crate) fn write_entity_ref<W: std::io::Write>(
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

pub(crate) struct EntityRefReader;

impl EntityRefReader {
    pub(crate) fn read<R: std::io::Read>(reader: &mut R) -> std::io::Result<Option<EntityRef>> {
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
}

fn read_entity_ref<R: std::io::Read>(reader: &mut R) -> std::io::Result<Option<EntityRef>> {
    EntityRefReader::read(reader)
}
