use graphdb_core::{StorageError, StorageResult};

pub(super) fn take_bytes(cursor: &mut &[u8], len: u32, field: &str) -> StorageResult<Vec<u8>> {
    let len = len as usize;
    if len > cursor.len() {
        return Err(StorageError::deserialize_error(format!(
            "{} length {} exceeds remaining input {}",
            field,
            len,
            cursor.len()
        )));
    }
    let (value, remaining) = cursor.split_at(len);
    *cursor = remaining;
    Ok(value.to_vec())
}
