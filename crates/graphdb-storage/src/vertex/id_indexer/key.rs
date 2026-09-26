//! Key types for the primary-key index: the external `IdKey` union and
//! the shape validation every mutation path shares.

use graphdb_core::error::{StorageError, StorageResult};
use graphdb_core::types::VERTEX_ID_MAX_SIZE;

pub(super) const ID_KEY_TYPE_INT: u8 = 0;
pub(super) const ID_KEY_TYPE_TEXT: u8 = 1;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum IdKey {
    Int(i64),
    Text(String),
}

impl IdKey {
    /// Write the key bytes into an existing buffer to avoid extra allocations.
    /// The buffer is cleared before writing.
    pub fn write_to(&self, buf: &mut Vec<u8>) {
        buf.clear();
        match self {
            IdKey::Int(val) => {
                buf.reserve(9);
                buf.push(ID_KEY_TYPE_INT);
                buf.extend_from_slice(&val.to_be_bytes());
            }
            IdKey::Text(val) => {
                buf.reserve(1 + val.len());
                buf.push(ID_KEY_TYPE_TEXT);
                buf.extend_from_slice(val.as_bytes());
            }
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.is_empty() {
            return Err(StorageError::deserialize_error(
                "Empty IdKey bytes".to_string(),
            ));
        }

        match bytes[0] {
            ID_KEY_TYPE_INT => {
                if bytes.len() != 9 {
                    return Err(StorageError::deserialize_error(format!(
                        "Invalid Int IdKey length: {}",
                        bytes.len()
                    )));
                }
                let val_bytes: [u8; 8] = bytes[1..9].try_into().map_err(|_| {
                    StorageError::deserialize_error("Invalid Int IdKey bytes".to_string())
                })?;
                Ok(IdKey::Int(i64::from_be_bytes(val_bytes)))
            }
            ID_KEY_TYPE_TEXT => {
                let text = String::from_utf8(bytes[1..].to_vec())
                    .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
                Ok(IdKey::Text(text))
            }
            tag => Err(StorageError::deserialize_error(format!(
                "Unknown IdKey type tag: {}",
                tag
            ))),
        }
    }
}

impl std::fmt::Display for IdKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdKey::Int(val) => write!(f, "{}", val),
            IdKey::Text(val) => write!(f, "{}", val),
        }
    }
}

/// Primary-key shape shared by every index mutation path.
///
/// The table layer validates the same shape, but the indexer enforces it
/// again so direct index users and persisted-file replays cannot smuggle in
/// over-long text keys or negative integer keys.
pub(super) fn validate_key_shape(key: &IdKey) -> StorageResult<()> {
    match key {
        IdKey::Int(id) if *id < 0 => Err(StorageError::invalid_input(format!(
            "Vertex id cannot be negative: {}",
            id
        ))),
        IdKey::Text(id) if id.len() > VERTEX_ID_MAX_SIZE => {
            Err(StorageError::invalid_input(format!(
                "Vertex id exceeds max length of {} bytes: got {} bytes",
                VERTEX_ID_MAX_SIZE,
                id.len()
            )))
        }
        _ => Ok(()),
    }
}
