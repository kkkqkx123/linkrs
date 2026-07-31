//! Write-ahead log for incremental index persistence.
//!
//! Each shard maintains a WAL file that records changes (insert/mark_deleted)
//! since the last checkpoint. On checkpoint, the full state is written to the
//! data files and the WAL is truncated.

use crate::core::types::Timestamp;
use crate::core::{StorageError, StorageResult};
use crate::storage::index::entity_ref_codec::{write_entity_ref, EntityRefReader};
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::types::IndexRecord;
use std::io::Write;
use std::path::Path;

const WAL_MAGIC: [u8; 4] = *b"INDW";
const WAL_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) enum WalEntry {
    Insert {
        is_forward: bool,
        key: SecondaryIndexKey,
        record: IndexRecord,
    },
    MarkDeleted {
        is_forward: bool,
        key: SecondaryIndexKey,
        deleted_ts: Timestamp,
    },
}

impl WalEntry {
    pub(crate) fn serialize_into<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        match self {
            WalEntry::Insert {
                is_forward,
                key,
                record,
            } => {
                writer.write_all(&[1u8])?;
                writer.write_all(&[*is_forward as u8])?;
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
                        let value_bytes = crate::core::value::ordered_codec::OrderedCodec::new()
                            .encode(value)
                            .map_err(|e| {
                                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                            })?;
                        writer.write_all(&(value_bytes.len() as u32).to_le_bytes())?;
                        writer.write_all(&value_bytes)?;
                    }
                }
                write_entity_ref(writer, &record.entity_ref)?;
            }
            WalEntry::MarkDeleted {
                is_forward,
                key,
                deleted_ts,
            } => {
                writer.write_all(&[2u8])?;
                writer.write_all(&[*is_forward as u8])?;
                writer.write_all(&(key.len() as u32).to_le_bytes())?;
                writer.write_all(key)?;
                writer.write_all(&deleted_ts.to_le_bytes())?;
            }
        }
        Ok(())
    }

    pub(crate) fn deserialize_from<R: std::io::Read>(
        reader: &mut R,
    ) -> std::io::Result<Option<Self>> {
        let mut tag = [0u8; 1];
        if reader.read_exact(&mut tag).is_err() {
            return Ok(None);
        }
        let mut dir = [0u8; 1];
        reader.read_exact(&mut dir)?;
        let is_forward = dir[0] != 0;

        let mut key_len_bytes = [0u8; 4];
        reader.read_exact(&mut key_len_bytes)?;
        let key_len = u32::from_le_bytes(key_len_bytes) as usize;
        let mut key = vec![0u8; key_len];
        reader.read_exact(&mut key)?;

        match tag[0] {
            1 => {
                let mut created_ts_bytes = [0u8; 8];
                reader.read_exact(&mut created_ts_bytes)?;
                let created_ts = u64::from_le_bytes(created_ts_bytes);

                let mut has_deleted = [0u8; 1];
                reader.read_exact(&mut has_deleted)?;
                let deleted_ts = if has_deleted[0] == 1 {
                    let mut deleted_ts_bytes = [0u8; 8];
                    reader.read_exact(&mut deleted_ts_bytes)?;
                    Some(u64::from_le_bytes(deleted_ts_bytes))
                } else {
                    None
                };

                let mut has_ev = [0u8; 1];
                reader.read_exact(&mut has_ev)?;
                let entity_version = if has_ev[0] == 1 {
                    let mut ev_bytes = [0u8; 8];
                    reader.read_exact(&mut ev_bytes)?;
                    Some(u64::from_le_bytes(ev_bytes))
                } else {
                    None
                };

                let mut num_included_bytes = [0u8; 4];
                reader.read_exact(&mut num_included_bytes)?;
                let num_included = u32::from_le_bytes(num_included_bytes) as usize;
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

                    let mut value_len_bytes = [0u8; 4];
                    reader.read_exact(&mut value_len_bytes)?;
                    let value_len = u32::from_le_bytes(value_len_bytes) as usize;
                    let mut value_bytes = vec![0u8; value_len];
                    reader.read_exact(&mut value_bytes)?;
                    let value = crate::core::value::ordered_codec::OrderedCodec::new()
                        .decode(&value_bytes)
                        .map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                        })?;
                    included_columns.push((name, value));
                }

                let entity_ref = EntityRefReader::read(reader)?;

                Ok(Some(WalEntry::Insert {
                    is_forward,
                    key,
                    record: IndexRecord {
                        created_ts,
                        deleted_ts,
                        entity_version,
                        included_columns: Some(included_columns),
                        entity_ref,
                    },
                }))
            }
            2 => {
                let mut deleted_ts_bytes = [0u8; 8];
                reader.read_exact(&mut deleted_ts_bytes)?;
                let deleted_ts = u64::from_le_bytes(deleted_ts_bytes);

                Ok(Some(WalEntry::MarkDeleted {
                    is_forward,
                    key,
                    deleted_ts,
                }))
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unknown WAL entry tag: {}", tag[0]),
            )),
        }
    }
}

pub(crate) fn write_wal_header<W: Write>(writer: &mut W) -> std::io::Result<()> {
    writer.write_all(&WAL_MAGIC)?;
    writer.write_all(&WAL_VERSION.to_le_bytes())?;
    Ok(())
}

pub(crate) fn read_and_validate_wal_header<R: std::io::Read>(
    reader: &mut R,
) -> std::io::Result<()> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != WAL_MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "WAL file magic mismatch",
        ));
    }
    let mut version_bytes = [0u8; 4];
    reader.read_exact(&mut version_bytes)?;
    let version = u32::from_le_bytes(version_bytes);
    if version != WAL_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unsupported WAL version: {}", version),
        ));
    }
    Ok(())
}

/// Append a WAL entry to the WAL file.
pub(crate) fn append_wal_entry<P: AsRef<Path>>(wal_path: P, entry: &WalEntry) -> StorageResult<()> {
    let path = wal_path.as_ref();
    let is_new = !path.exists();

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    let mut writer = std::io::BufWriter::new(file);
    if is_new {
        write_wal_header(&mut writer).map_err(|e| StorageError::io_error(e.to_string()))?;
    }
    entry
        .serialize_into(&mut writer)
        .map_err(|e| StorageError::io_error(e.to_string()))?;
    writer
        .flush()
        .map_err(|e| StorageError::io_error(e.to_string()))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| StorageError::io_error(e.to_string()))?;
    Ok(())
}

/// Read all WAL entries from a WAL file.
pub(crate) fn read_wal_entries<P: AsRef<Path>>(wal_path: P) -> StorageResult<Vec<WalEntry>> {
    let path = wal_path.as_ref();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    read_and_validate_wal_header(&mut reader).map_err(|e| StorageError::io_error(e.to_string()))?;

    let mut entries = Vec::new();
    loop {
        match WalEntry::deserialize_from(&mut reader) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => break,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(StorageError::io_error(e.to_string())),
        }
    }
    Ok(entries)
}

/// Truncate the WAL file (called after checkpoint).
pub(crate) fn truncate_wal<P: AsRef<Path>>(wal_path: P) -> StorageResult<()> {
    let path = wal_path.as_ref();
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::wal::EntityRef;
    use crate::core::Value;

    #[test]
    fn serialize_insert_roundtrip() {
        let record = IndexRecord::new(100)
            .with_entity_ref(EntityRef::Vertex(VertexId::from_int64(42)))
            .with_entity_version(50);
        let entry = WalEntry::Insert {
            is_forward: true,
            key: vec![1, 2, 3, 4],
            record,
        };

        let mut buf = Vec::new();
        entry.serialize_into(&mut buf).unwrap();
        let mut reader = std::io::Cursor::new(&buf);
        let decoded = WalEntry::deserialize_from(&mut reader).unwrap().unwrap();

        match decoded {
            WalEntry::Insert {
                is_forward,
                key,
                record,
            } => {
                assert!(is_forward);
                assert_eq!(key, vec![1, 2, 3, 4]);
                assert_eq!(record.created_ts, 100);
                assert_eq!(record.entity_version, Some(50));
            }
            _ => panic!("Expected Insert entry"),
        }
    }

    #[test]
    fn serialize_mark_deleted_roundtrip() {
        let entry = WalEntry::MarkDeleted {
            is_forward: false,
            key: vec![5, 6, 7],
            deleted_ts: 200,
        };

        let mut buf = Vec::new();
        entry.serialize_into(&mut buf).unwrap();
        let mut reader = std::io::Cursor::new(&buf);
        let decoded = WalEntry::deserialize_from(&mut reader).unwrap().unwrap();

        match decoded {
            WalEntry::MarkDeleted {
                is_forward,
                key,
                deleted_ts,
            } => {
                assert!(!is_forward);
                assert_eq!(key, vec![5, 6, 7]);
                assert_eq!(deleted_ts, 200);
            }
            _ => panic!("Expected MarkDeleted entry"),
        }
    }

    #[test]
    fn serialize_with_included_columns() {
        let record =
            IndexRecord::new_with_columns(100, vec![("name".to_string(), Value::string("Alice"))]);
        let entry = WalEntry::Insert {
            is_forward: true,
            key: vec![1, 2, 3],
            record,
        };

        let mut buf = Vec::new();
        entry.serialize_into(&mut buf).unwrap();
        let mut reader = std::io::Cursor::new(&buf);
        let decoded = WalEntry::deserialize_from(&mut reader).unwrap().unwrap();

        match decoded {
            WalEntry::Insert { record, .. } => {
                let columns = record.included_columns.unwrap();
                assert_eq!(columns.len(), 1);
                assert_eq!(columns[0].0, "name");
                assert_eq!(columns[0].1, Value::string("Alice"));
            }
            _ => panic!("Expected Insert entry"),
        }
    }

    #[test]
    fn wal_file_roundtrip() {
        let temp_dir = std::env::temp_dir();
        let wal_path = temp_dir.join("test_wal.bin");

        // Clean up any existing file
        let _ = std::fs::remove_file(&wal_path);

        let entries = vec![
            WalEntry::Insert {
                is_forward: true,
                key: vec![1, 2, 3],
                record: IndexRecord::new(100),
            },
            WalEntry::MarkDeleted {
                is_forward: false,
                key: vec![4, 5, 6],
                deleted_ts: 200,
            },
        ];

        for entry in &entries {
            append_wal_entry(&wal_path, entry).unwrap();
        }

        let loaded = read_wal_entries(&wal_path).unwrap();
        assert_eq!(loaded.len(), 2);

        truncate_wal(&wal_path).unwrap();
        assert!(!wal_path.exists());

        // Clean up
        let _ = std::fs::remove_file(&wal_path);
    }

    #[test]
    fn read_empty_wal_returns_empty() {
        let temp_dir = std::env::temp_dir();
        let wal_path = temp_dir.join("nonexistent_wal.bin");

        let _ = std::fs::remove_file(&wal_path);
        let entries = read_wal_entries(&wal_path).unwrap();
        assert!(entries.is_empty());
    }
}
