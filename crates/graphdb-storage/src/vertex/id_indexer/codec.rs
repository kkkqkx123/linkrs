//! Persistence codecs for the primary-key index: the full baseline
//! snapshot and the since-baseline delta log.

use std::io::Read;

use graphdb_core::error::StorageResult;

use super::config::IdIndexerConfig;
use super::key::{validate_key_shape, IdKey};
use super::manager::{IdManager, IndexDelta, SharedCore};

/// Serialize the since-baseline delta for `id_indexer.delta`.
///
/// Entry encoding reuses the key bytes ([`IdKey::write_to`]): `count:u32`
/// followed by per-entry `op:u8` (`0` insert with `id:u32`, `1` remove)
/// plus `key_len:u32` and key bytes. No new key format is introduced.
/// Core-only snapshot of the delta log.
pub(super) fn serialize_delta(core: &SharedCore) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(core.delta_log.len() as u32).to_le_bytes());
    let mut key_buf = Vec::new();
    for delta in &core.delta_log {
        match delta {
            IndexDelta::Insert { key, id } => {
                buf.push(0u8);
                buf.extend_from_slice(&id.to_le_bytes());
                key.write_to(&mut key_buf);
                buf.extend_from_slice(&(key_buf.len() as u32).to_le_bytes());
                buf.extend_from_slice(&key_buf);
            }
            IndexDelta::Remove { key } => {
                buf.push(1u8);
                key.write_to(&mut key_buf);
                buf.extend_from_slice(&(key_buf.len() as u32).to_le_bytes());
                buf.extend_from_slice(&key_buf);
            }
        }
    }
    buf
}

/// Decode delta entries. Corrupt bytes fail the whole delta so the
/// caller refuses the open.
pub fn deserialize_delta(data: &[u8]) -> StorageResult<Vec<(u8, u32, IdKey)>> {
    let mut cursor = data;
    let take = |cursor: &mut &[u8], len: usize, field: &str| -> StorageResult<Vec<u8>> {
        if len > cursor.len() {
            return Err(graphdb_core::error::StorageError::deserialize_error(
                format!(
                    "pk delta {} length {} exceeds remaining {}",
                    field,
                    len,
                    cursor.len()
                ),
            ));
        }
        let (head, tail) = cursor.split_at(len);
        *cursor = tail;
        Ok(head.to_vec())
    };
    if cursor.len() < 4 {
        return Err(graphdb_core::error::StorageError::deserialize_error(
            "pk delta truncated count".to_string(),
        ));
    }
    let count = u32::from_le_bytes(take(&mut cursor, 4, "count")?[..4].try_into().map_err(
        |_| {
            graphdb_core::error::StorageError::deserialize_error(
                "pk delta count malformed".to_string(),
            )
        },
    )?) as usize;
    // Sanity bound mirroring the baseline check: every entry needs at
    // least its op, key length, and key tag byte on the wire, so a count
    // the remaining bytes cannot hold is corruption rather than data.
    const MIN_DELTA_ENTRY_BYTES: usize = 6;
    if count > cursor.len() / MIN_DELTA_ENTRY_BYTES {
        return Err(graphdb_core::error::StorageError::deserialize_error(
            format!(
                "pk delta count {} exceeds wire capacity of {} bytes",
                count,
                cursor.len(),
            ),
        ));
    }
    let mut out = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        let op = take(&mut cursor, 1, "op")?[0];
        if op != 0 && op != 1 {
            return Err(graphdb_core::error::StorageError::deserialize_error(
                format!("pk delta unknown op {}", op),
            ));
        }
        let id = if op == 0 {
            u32::from_le_bytes(take(&mut cursor, 4, "id")?[..4].try_into().map_err(|_| {
                graphdb_core::error::StorageError::deserialize_error(
                    "pk delta id malformed".to_string(),
                )
            })?)
        } else {
            0
        };
        let key_len = u32::from_le_bytes(take(&mut cursor, 4, "key_len")?[..4].try_into().map_err(
            |_| {
                graphdb_core::error::StorageError::deserialize_error(
                    "pk delta key_len malformed".to_string(),
                )
            },
        )?) as usize;
        let key_bytes = take(&mut cursor, key_len, "key")?;
        out.push((op, id, IdKey::from_bytes(&key_bytes)?));
    }
    if !cursor.is_empty() {
        return Err(graphdb_core::error::StorageError::deserialize_error(
            format!("pk delta has {} trailing bytes", cursor.len()),
        ));
    }
    Ok(out)
}

/// Serialize the index to bytes for persistence.
///
/// Format:
/// - count: u32 (number of entries)
/// - for each entry:
///   - internal_id: u32
///   - key_len: u32
///   - key_bytes: [u8; key_len]
pub(super) fn serialize(core: &SharedCore) -> Vec<u8> {
    let mut buf = Vec::new();
    let count = core.live_ids.len() as u32;
    buf.extend_from_slice(&count.to_le_bytes());

    let mut key_buf = Vec::new();
    for (idx, key_opt) in core.keys.iter().enumerate() {
        if let Some(key) = key_opt {
            buf.extend_from_slice(&(idx as u32).to_le_bytes());
            key.write_to(&mut key_buf);
            buf.extend_from_slice(&(key_buf.len() as u32).to_le_bytes());
            buf.extend_from_slice(&key_buf);
        }
    }

    buf
}

/// Deserialize from bytes, rebuilding the index.
pub fn deserialize(data: &[u8]) -> StorageResult<IdManager> {
    let mut cursor = data;
    let mut count_bytes = [0u8; 4];
    cursor.read_exact(&mut count_bytes)?;
    let count = u32::from_le_bytes(count_bytes) as usize;

    // Sanity bound: every entry needs at least its id, key length, and
    // key tag byte on the wire, so a count the remaining bytes cannot
    // hold is corruption rather than data. This also bounds the
    // pre-allocation below by the file size.
    const MIN_ENTRY_BYTES: usize = 9;
    if count > data.len().saturating_sub(4) / MIN_ENTRY_BYTES {
        return Err(graphdb_core::error::StorageError::deserialize_error(
            format!(
                "pk baseline count {} exceeds wire capacity of {} bytes",
                count,
                data.len(),
            ),
        ));
    }

    let manager = IdManager::with_config(IdIndexerConfig::default());
    manager.reserve(count);

    for _ in 0..count {
        let mut id_bytes = [0u8; 4];
        cursor.read_exact(&mut id_bytes)?;
        let internal_id = u32::from_le_bytes(id_bytes);

        let mut key_len_bytes = [0u8; 4];
        cursor.read_exact(&mut key_len_bytes)?;
        let key_len = u32::from_le_bytes(key_len_bytes) as usize;
        let mut key_bytes = vec![0u8; key_len];
        cursor.read_exact(&mut key_bytes)?;

        let key = IdKey::from_bytes(&key_bytes)?;
        validate_key_shape(&key)?;
        manager.set_at(internal_id, key);
    }

    // Baselines hold each live key exactly once; fewer bindings than
    // declared means duplicated or colliding keys in a corrupt file.
    if manager.len() != count {
        return Err(graphdb_core::error::StorageError::deserialize_error(
            format!(
                "pk baseline holds {} bindings for declared count {}",
                manager.len(),
                count
            ),
        ));
    }

    // Rebuild free list for holes left by non-dense persisted ids
    // (e.g., after deletions that left gaps).
    {
        let mut core = manager.core.lock();
        core.free_ids = core
            .keys
            .iter()
            .enumerate()
            .filter_map(|(idx, k)| if k.is_none() { Some(idx as u32) } else { None })
            .collect();
    }

    Ok(manager)
}
