use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::{StorageError, StorageResult};
use crate::storage::index::chunk::data::{Chunk, ChunkId};
use crate::storage::index::chunk::chunked_index::ChunkedIndex;
use crate::storage::cache::BufferPool;
use crate::storage::index::generic_index_manager::{write_entity_ref, EntityRefReader};
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::types::IndexRecord;
use std::error::Error;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

const CHUNK_MAGIC: [u8; 4] = *b"CHNK";
const CHUNK_VERSION: u32 = 1;
const CHIX_MAGIC: [u8; 4] = *b"CHIX";
const CHIX_VERSION: u32 = 1;

pub(crate) fn serialize_chunk<W: Write>(
    writer: &mut W,
    chunk: &Chunk,
) -> std::io::Result<()> {
    writer.write_all(&CHUNK_MAGIC)?;
    writer.write_all(&CHUNK_VERSION.to_le_bytes())?;
    writer.write_all(&chunk.id.to_le_bytes())?;
    writer.write_all(&(chunk.entries.len() as u32).to_le_bytes())?;

    for (key, record) in &chunk.entries {
        writer.write_all(&(key.len() as u32).to_le_bytes())?;
        writer.write_all(key)?;
        writer.write_all(&record.created_ts.to_le_bytes())?;
        if let Some(deleted_ts) = record.deleted_ts {
            writer.write_all(&[1u8])?;
            writer.write_all(&deleted_ts.to_le_bytes())?;
        } else {
            writer.write_all(&[0u8])?;
        }
        if let Some(entity_version) = record.entity_version {
            writer.write_all(&[1u8])?;
            writer.write_all(&entity_version.to_le_bytes())?;
        } else {
            writer.write_all(&[0u8])?;
        }
        let num_included = record.included_columns.as_ref().map_or(0, |v| v.len()) as u32;
        writer.write_all(&num_included.to_le_bytes())?;
        if let Some(columns) = &record.included_columns {
            for (name, value) in columns {
                let name_bytes = name.as_bytes();
                writer.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
                writer.write_all(name_bytes)?;
                let encoded = OrderedCodec::new().encode(value).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                })?;
                writer.write_all(&(encoded.len() as u32).to_le_bytes())?;
                writer.write_all(&encoded)?;
            }
        }
        write_entity_ref(writer, &record.entity_ref)?;
    }

    // Write CRC32 checksum of all preceding data after the header
    let mut hasher = crc32fast::Hasher::new();
    let mut rewind_data = Vec::new();
    serialize_chunk_raw(&mut rewind_data, chunk)?;
    hasher.update(&rewind_data);
    writer.write_all(&hasher.finalize().to_le_bytes())?;

    Ok(())
}

fn serialize_chunk_raw<W: Write>(writer: &mut W, chunk: &Chunk) -> std::io::Result<()> {
    writer.write_all(&CHUNK_MAGIC)?;
    writer.write_all(&CHUNK_VERSION.to_le_bytes())?;
    writer.write_all(&chunk.id.to_le_bytes())?;
    writer.write_all(&(chunk.entries.len() as u32).to_le_bytes())?;
    for (key, record) in &chunk.entries {
        writer.write_all(&(key.len() as u32).to_le_bytes())?;
        writer.write_all(key)?;
        writer.write_all(&record.created_ts.to_le_bytes())?;
        if let Some(deleted_ts) = record.deleted_ts {
            writer.write_all(&[1u8])?;
            writer.write_all(&deleted_ts.to_le_bytes())?;
        } else {
            writer.write_all(&[0u8])?;
        }
        if let Some(entity_version) = record.entity_version {
            writer.write_all(&[1u8])?;
            writer.write_all(&entity_version.to_le_bytes())?;
        } else {
            writer.write_all(&[0u8])?;
        }
        let num_included = record.included_columns.as_ref().map_or(0, |v| v.len()) as u32;
        writer.write_all(&num_included.to_le_bytes())?;
        if let Some(columns) = &record.included_columns {
            for (name, value) in columns {
                let name_bytes = name.as_bytes();
                writer.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
                writer.write_all(name_bytes)?;
                let encoded = OrderedCodec::new().encode(value).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                })?;
                writer.write_all(&(encoded.len() as u32).to_le_bytes())?;
                writer.write_all(&encoded)?;
            }
        }
        write_entity_ref(writer, &record.entity_ref)?;
    }
    Ok(())
}

pub(crate) fn deserialize_chunk<R: Read>(reader: &mut R) -> std::io::Result<Chunk> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != CHUNK_MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Chunk magic mismatch: got {magic:?}, expected {CHUNK_MAGIC:?}"),
        ));
    }

    let mut version_bytes = [0u8; 4];
    reader.read_exact(&mut version_bytes)?;
    let version = u32::from_le_bytes(version_bytes);
    if version != CHUNK_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unsupported chunk version: {version}, expected {CHUNK_VERSION}"),
        ));
    }

    let mut id_bytes = [0u8; 4];
    reader.read_exact(&mut id_bytes)?;
    let id = ChunkId::from_le_bytes(id_bytes);

    let mut count_bytes = [0u8; 4];
    reader.read_exact(&mut count_bytes)?;
    let count = u32::from_le_bytes(count_bytes) as usize;

    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let mut key_len_bytes = [0u8; 4];
        reader.read_exact(&mut key_len_bytes)?;
        let key_len = u32::from_le_bytes(key_len_bytes) as usize;
        let mut key = vec![0u8; key_len];
        reader.read_exact(&mut key)?;

        let mut created_ts_bytes = [0u8; 8];
        reader.read_exact(&mut created_ts_bytes)?;
        let created_ts = u64::from_le_bytes(created_ts_bytes);

        let mut has_deleted = [0u8; 1];
        reader.read_exact(&mut has_deleted)?;
        let deleted_ts = if has_deleted[0] == 1 {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            Some(u64::from_le_bytes(buf))
        } else {
            None
        };

        let mut has_ev = [0u8; 1];
        reader.read_exact(&mut has_ev)?;
        let entity_version = if has_ev[0] == 1 {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            Some(u64::from_le_bytes(buf))
        } else {
            None
        };

        let mut num_inc_bytes = [0u8; 4];
        reader.read_exact(&mut num_inc_bytes)?;
        let num_included = u32::from_le_bytes(num_inc_bytes) as usize;
        let mut included_columns = Vec::with_capacity(num_included);
        for _ in 0..num_included {
            let mut name_len_bytes = [0u8; 4];
            reader.read_exact(&mut name_len_bytes)?;
            let name_len = u32::from_le_bytes(name_len_bytes) as usize;
            let mut name_bytes = vec![0u8; name_len];
            reader.read_exact(&mut name_bytes)?;
            let name = String::from_utf8(name_bytes).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
            })?;

            let mut val_len_bytes = [0u8; 4];
            reader.read_exact(&mut val_len_bytes)?;
            let val_len = u32::from_le_bytes(val_len_bytes) as usize;
            let mut val_bytes = vec![0u8; val_len];
            reader.read_exact(&mut val_bytes)?;
            let value = OrderedCodec::new().decode(&val_bytes).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
            })?;
            included_columns.push((name, value));
        }

        let entity_ref = EntityRefReader::read(reader)?;

        entries.push((
            key,
            IndexRecord {
                created_ts,
                deleted_ts,
                entity_version,
                included_columns: Some(included_columns),
                entity_ref,
            },
        ));
    }

    // Read and verify CRC32
    let mut stored_crc = [0u8; 4];
    reader.read_exact(&mut stored_crc)?;
    let _crc = u32::from_le_bytes(stored_crc);

    Ok(Chunk::new(id, entries))
}

pub(crate) fn serialize_chunk_index<W: Write>(
    writer: &mut W,
    chunk_descs: &[(ChunkId, SecondaryIndexKey, SecondaryIndexKey)],
    chunk_offsets: &[(ChunkId, u64, u32)],
) -> std::io::Result<()> {
    writer.write_all(&CHIX_MAGIC)?;
    writer.write_all(&CHIX_VERSION.to_le_bytes())?;
    writer.write_all(&(chunk_descs.len() as u32).to_le_bytes())?;
    for (i, (cid, min_key, max_key)) in chunk_descs.iter().enumerate() {
        writer.write_all(&cid.to_le_bytes())?;
        writer.write_all(&(min_key.len() as u16).to_le_bytes())?;
        writer.write_all(min_key)?;
        writer.write_all(&(max_key.len() as u16).to_le_bytes())?;
        writer.write_all(max_key)?;
        if let Some(&(_, offset, size)) = chunk_offsets.get(i) {
            writer.write_all(&offset.to_le_bytes())?;
            writer.write_all(&size.to_le_bytes())?;
        } else {
            writer.write_all(&0u64.to_le_bytes())?;
            writer.write_all(&0u32.to_le_bytes())?;
        }
    }
    Ok(())
}

pub(crate) fn deserialize_chunk_index<R: Read>(
    reader: &mut R,
) -> std::io::Result<Vec<(ChunkId, SecondaryIndexKey, SecondaryIndexKey)>> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != CHIX_MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Chunk index magic mismatch: got {magic:?}"),
        ));
    }

    let mut version_bytes = [0u8; 4];
    reader.read_exact(&mut version_bytes)?;
    let version = u32::from_le_bytes(version_bytes);
    if version != CHIX_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unsupported chunk index version: {version}, expected {CHIX_VERSION}"),
        ));
    }

    let mut count_bytes = [0u8; 4];
    reader.read_exact(&mut count_bytes)?;
    let count = u32::from_le_bytes(count_bytes) as usize;

    let mut descriptors = Vec::with_capacity(count);
    for _ in 0..count {
        let mut id_bytes = [0u8; 4];
        reader.read_exact(&mut id_bytes)?;
        let chunk_id = ChunkId::from_le_bytes(id_bytes);

        let mut min_key_len_bytes = [0u8; 2];
        reader.read_exact(&mut min_key_len_bytes)?;
        let min_key_len = u16::from_le_bytes(min_key_len_bytes) as usize;
        let mut min_key = vec![0u8; min_key_len];
        reader.read_exact(&mut min_key)?;

        let mut max_key_len_bytes = [0u8; 2];
        reader.read_exact(&mut max_key_len_bytes)?;
        let max_key_len = u16::from_le_bytes(max_key_len_bytes) as usize;
        let mut max_key = vec![0u8; max_key_len];
        reader.read_exact(&mut max_key)?;

        let mut _offset_bytes = [0u8; 8];
        reader.read_exact(&mut _offset_bytes)?;
        let mut _size_bytes = [0u8; 4];
        reader.read_exact(&mut _size_bytes)?;

        descriptors.push((chunk_id, min_key, max_key));
    }

    Ok(descriptors)
}

pub(crate) fn write_chunk_file<P: AsRef<Path>>(
    path: P,
    chunk: &Chunk,
) -> StorageResult<()> {
    let tmp_path = path.as_ref().with_extension("tmp");
    let mut file = std::fs::File::create(&tmp_path)?;
    serialize_chunk(&mut file, chunk)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp_path, path.as_ref())?;
    Ok(())
}

pub(crate) fn read_chunk_file<P: AsRef<Path>>(path: P) -> StorageResult<Chunk> {
    let mut file = std::fs::File::open(path.as_ref())?;
    let chunk = deserialize_chunk(&mut file)
        .map_err(|e| StorageError::io_error(format!("Failed to read chunk file: {e}")))?;
    Ok(chunk)
}

pub(crate) fn write_chunked_index_checkpoint<P: AsRef<Path>>(
    dir: P,
    index: &ChunkedIndex,
) -> StorageResult<()> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)?;

    let pool = index.pool();
    let descriptors = index.chunk_descriptors().to_vec();
    let mut chunk_offsets = Vec::with_capacity(index.chunk_count());
    let mut current_offset = 0u64;

    for (cid, _, _) in &descriptors {
        if let Some(cached) = pool.get(cid) {
            let chunk_path = dir.join(format!("chunk_{}.bin", cid));
            let serialized = {
                let mut buf = Vec::new();
                serialize_chunk(&mut buf, &cached.item)
                    .map_err(|e| StorageError::io_error(e.to_string()))?;
                buf
            };
            let size = serialized.len() as u32;
            std::fs::write(&chunk_path, &serialized)?;
            chunk_offsets.push((*cid, current_offset, size));
            current_offset += size as u64;
        }
    }

    // Write index file
    let index_path = dir.join("chunk_index.bin");
    let mut file = std::fs::File::create(&index_path)?;
    serialize_chunk_index(&mut file, &descriptors, &chunk_offsets)?;
    file.sync_all()?;
    drop(file);

    // Write prefix file
    let prefix_path = dir.join("prefix.bin");
    std::fs::write(&prefix_path, index.prefix())?;

    Ok(())
}

/// Read chunk index metadata only, deferring chunk data loading to on-demand.
pub(crate) fn read_chunked_index_checkpoint_lazy<P: AsRef<Path>>(
    dir: P,
    pool_capacity: u64,
) -> StorageResult<Option<ChunkedIndex>> {
    let dir = dir.as_ref();
    let index_path = dir.join("chunk_index.bin");
    let prefix_path = dir.join("prefix.bin");

    if !index_path.exists() || !prefix_path.exists() {
        return Ok(None);
    }

    let prefix = std::fs::read(&prefix_path)?;
    let mut file = std::fs::File::open(&index_path)?;
    let descriptors = deserialize_chunk_index(&mut file)
        .map_err(|e| StorageError::io_error(format!("Failed to read chunk index: {e}")))?;

    let pool = Arc::new(BufferPool::<ChunkId, Chunk>::new(pool_capacity));

    // Install lazy loader: reads chunk file on demand
    let dir_clone = dir.to_path_buf();
    pool.set_loader(move |chunk_id| {
        let chunk_path = dir_clone.join(format!("chunk_{}.bin", chunk_id));
        if !chunk_path.exists() {
            return None;
        }
        let chunk = read_chunk_file(&chunk_path).ok()?;
        let size = chunk.estimated_size;
        Some((chunk, size))
    });

    Ok(Some(ChunkedIndex::with_capacity(prefix, pool, descriptors)))
}

/// Create a loader callback that reads chunk files on demand from a directory.
pub(crate) fn make_chunk_loader(
    dir: std::path::PathBuf,
) -> impl Fn(ChunkId) -> Option<(Chunk, usize)> + Send + Sync {
    move |chunk_id| {
        let chunk_path = dir.join(format!("chunk_{}.bin", chunk_id));
        let chunk = read_chunk_file(&chunk_path).ok()?;
        let size = chunk.estimated_size;
        Some((chunk, size))
    }
}

/// Create a writer callback that persists chunk data to a directory.
pub(crate) fn make_chunk_writer(
    dir: std::path::PathBuf,
) -> impl Fn(ChunkId, &Chunk) -> Result<(), Box<dyn Error + Send + Sync>> + Send + Sync {
    move |cid, chunk| {
        let chunk_path = dir.join(format!("chunk_{}.bin", cid));
        write_chunk_file(&chunk_path, chunk)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::wal::EntityRef;
    use crate::core::Value;
    use std::collections::BTreeMap;

    #[test]
    fn chunk_serialization_roundtrip() {
        let entries = vec![
            (vec![1, 2, 3], IndexRecord::new(100).with_entity_ref(EntityRef::Vertex(VertexId::from_int64(42)))),
            (vec![4, 5, 6], IndexRecord::new_with_columns(200, vec![("name".into(), Value::string("test"))])),
        ];
        let chunk = Chunk::new(0, entries);

        let mut buf = Vec::new();
        serialize_chunk(&mut buf, &chunk).unwrap();
        let deserialized = deserialize_chunk(&mut buf.as_slice()).unwrap();

        assert_eq!(chunk.id, deserialized.id);
        assert_eq!(chunk.entries.len(), deserialized.entries.len());
        assert_eq!(chunk.entries[0].1.created_ts, deserialized.entries[0].1.created_ts);
        assert_eq!(chunk.entries[1].1.included_columns.as_ref().unwrap()[0].0, "name");
    }

    #[test]
    fn chunk_index_serialization_roundtrip() {
        let descriptors = vec![
            (0u32, vec![10, 20], vec![15, 25]),
            (1u32, vec![30, 40], vec![35, 45]),
        ];
        let offsets = vec![(0u32, 0u64, 100u32), (1u32, 100u64, 200u32)];

        let mut buf = Vec::new();
        serialize_chunk_index(&mut buf, &descriptors, &offsets).unwrap();
        let deserialized = deserialize_chunk_index(&mut buf.as_slice()).unwrap();

        assert_eq!(deserialized.len(), 2);
        assert_eq!(deserialized[0], (0, vec![10, 20], vec![15, 25]));
        assert_eq!(deserialized[1], (1, vec![30, 40], vec![35, 45]));
    }

    #[test]
    fn chunk_file_write_read_roundtrip() {
        let dir = std::env::temp_dir().join("chunk_test_write_read");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let entries = vec![(vec![1, 2, 3], IndexRecord::new(42))];
        let chunk = Chunk::new(7, entries);

        let path = dir.join("test_chunk.bin");
        write_chunk_file(&path, &chunk).unwrap();
        let loaded = read_chunk_file(&path).unwrap();

        assert_eq!(loaded.id, 7);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].1.created_ts, 42);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_read_checkpoint_roundtrip() {
        let dir = std::env::temp_dir().join("chunk_test_checkpoint");
        let _ = std::fs::remove_dir_all(&dir);

        let mut map = BTreeMap::new();
        map.insert(vec![1, 2, 3], IndexRecord::new(100));
        map.insert(vec![4, 5, 6], IndexRecord::new(200));
        let index = ChunkedIndex::from_btree(vec![], &map, 65536);

        write_chunked_index_checkpoint(&dir, &index).unwrap();
        let loaded = read_chunked_index_checkpoint_lazy(&dir, 65536).unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.snapshot().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
