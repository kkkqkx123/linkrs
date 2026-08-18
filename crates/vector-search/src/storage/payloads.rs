//! `payloads.bin` — per-slot payload records with a tombstone flag.

use std::path::Path;
use std::sync::Arc;

use crate::error::{Result, VectorSearchError};
use crate::storage::directory::{BlobDirectory, DirView, FLAG_TOMBSTONE, SLOT_REC_SIZE};
use crate::types::Payload;

const MAGIC: [u8; 4] = *b"VPLD";

/// Per-slot payload blobs with tombstone flags, backed by `payloads.bin`.
pub struct Payloads {
    dir: BlobDirectory,
}

impl Payloads {
    pub fn create(path: &Path, initial_capacity: u64) -> Result<Self> {
        Ok(Self {
            dir: BlobDirectory::create(path, MAGIC, SLOT_REC_SIZE, initial_capacity)?,
        })
    }

    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            dir: BlobDirectory::open(path, MAGIC, SLOT_REC_SIZE)?,
        })
    }

    pub fn snapshot(&self) -> arc_swap::Guard<Arc<DirView>> {
        self.dir.snapshot()
    }

    pub fn grow_to(&self, target_capacity: u64) -> Result<()> {
        self.dir.grow_to(target_capacity)
    }

    /// Atomically replace the backing file with a compacted `tmp_path`.
    pub fn replace_from(&self, tmp_path: &Path) -> Result<()> {
        self.dir.replace_from(tmp_path)
    }

    /// Store the payload for a slot (appends a new blob, orphans the old one).
    pub fn append_payload(&self, slot: usize, payload: Option<&Payload>) -> Result<()> {
        match payload {
            Some(payload) => {
                // `Payload` is `HashMap<String, serde_json::Value>`; postcard
                // cannot encode untagged enums, so payloads are JSON blobs.
                let bytes = serde_json::to_vec(payload)?;
                self.dir.append_blob(slot, &bytes, 0)
            }
            None => {
                // Zero-length record; keep existing flags untouched.
                self.dir.append_blob(slot, &[], 0)
            }
        }
    }

    /// Read the payload for a slot from a snapshot.
    pub fn read_payload(view: &DirView, slot: usize) -> Result<Option<Payload>> {
        match view.blob(SLOT_REC_SIZE, slot) {
            Some(bytes) if !bytes.is_empty() => {
                let payload = serde_json::from_slice(bytes).map_err(|e| {
                    VectorSearchError::CorruptData(format!("bad payload blob: {e}"))
                })?;
                Ok(Some(payload))
            }
            _ => Ok(None),
        }
    }

    /// Mark a slot tombstoned (`true`) or alive (`false`).
    pub fn set_tombstone(&self, slot: usize, tombstoned: bool) -> Result<()> {
        let flags = if tombstoned { FLAG_TOMBSTONE } else { 0 };
        self.dir.set_flags(slot, flags)
    }

    pub fn is_tombstoned(view: &DirView, slot: usize) -> bool {
        view.flags(SLOT_REC_SIZE, slot)
            .map(|f| f & FLAG_TOMBSTONE != 0)
            .unwrap_or(false)
    }
}
