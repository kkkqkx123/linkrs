//! Column-level large-object overflow area.
//!
//! Payloads above the overflow threshold live outside the main column file
//! in a `<col>.overflow` sidecar. In memory the store is an append-only
//! buffer; flush persists it and load rebuilds the index. Deletes never
//! reclaim entries eagerly; flush rebuilds the file from live rows only.

use std::io::Read;

use graphdb_core::{StorageError, StorageResult};

/// Default payload size above which a string spills to the overflow file.
pub const DEFAULT_OVERFLOW_THRESHOLD: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OverflowHandle {
    pub entry_id: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct OverflowEntry {
    pub offset: u64,
    pub len: u32,
}

#[derive(Debug, Clone, Default)]
pub struct OverflowStore {
    pending: Vec<u8>,
    index: Vec<OverflowEntry>,
    threshold: usize,
}

impl OverflowStore {
    pub fn new(threshold: usize) -> Self {
        Self {
            pending: Vec::new(),
            index: Vec::new(),
            threshold,
        }
    }

    pub fn set_threshold(&mut self, threshold: usize) {
        self.threshold = threshold;
    }

    pub fn should_overflow(&self, len: usize) -> bool {
        len > self.threshold
    }

    pub fn append(&mut self, bytes: &[u8]) -> OverflowHandle {
        let offset = self.pending.len() as u64;
        self.pending.extend_from_slice(bytes);
        let entry_id = self.index.len() as u32;
        self.index.push(OverflowEntry {
            offset,
            len: bytes.len() as u32,
        });
        OverflowHandle { entry_id }
    }

    pub fn get(&self, handle: &OverflowHandle) -> Option<Vec<u8>> {
        let entry = self.index.get(handle.entry_id as usize)?;
        let start = entry.offset as usize;
        let end = start + entry.len as usize;
        if end <= self.pending.len() {
            return Some(self.pending[start..end].to_vec());
        }
        None
    }

    pub fn memory_usage(&self) -> usize {
        self.pending.len() + self.index.len() * std::mem::size_of::<OverflowEntry>()
    }

    /// Rebuild from live payloads only; used by flush to drop garbage.
    pub fn rebuild_from_live(&mut self, live: &[Vec<u8>]) {
        self.pending.clear();
        self.index.clear();
        for payload in live {
            self.append(payload);
        }
    }

    pub fn flush_to_sidecar_buffer(&self, buf: &mut Vec<u8>) -> StorageResult<()> {
        let start = buf.len();
        buf.extend_from_slice(&(self.index.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.threshold as u32).to_le_bytes());
        for entry in &self.index {
            buf.extend_from_slice(&entry.offset.to_le_bytes());
            buf.extend_from_slice(&entry.len.to_le_bytes());
        }
        buf.extend_from_slice(&self.pending);
        let crc = crc32fast::hash(&buf[start..]);
        buf.extend_from_slice(&crc.to_le_bytes());
        Ok(())
    }

    pub fn load_from_bytes(&mut self, bytes: &[u8]) -> StorageResult<()> {
        if bytes.len() < 12 {
            return Err(StorageError::deserialize_error(
                "overflow section too small".to_string(),
            ));
        }
        let stored_crc = u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().map_err(|_| {
            StorageError::deserialize_error("overflow section CRC tail malformed".to_string())
        })?);
        let computed = crc32fast::hash(&bytes[..bytes.len() - 4]);
        if stored_crc != computed {
            return Err(StorageError::deserialize_error(format!(
                "overflow section CRC mismatch: stored={:#x} computed={:#x}",
                stored_crc, computed
            )));
        }
        let mut cursor = &bytes[..bytes.len() - 4];
        let mut u32b = [0u8; 4];
        cursor.read_exact(&mut u32b)?;
        let count = u32::from_le_bytes(u32b) as usize;
        cursor.read_exact(&mut u32b)?;
        self.threshold = u32::from_le_bytes(u32b) as usize;
        self.index.clear();
        for _ in 0..count {
            let mut off = [0u8; 8];
            cursor.read_exact(&mut off)?;
            cursor.read_exact(&mut u32b)?;
            self.index.push(OverflowEntry {
                offset: u64::from_le_bytes(off),
                len: u32::from_le_bytes(u32b),
            });
        }
        self.pending = cursor.to_vec();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_get_roundtrip() {
        let mut store = OverflowStore::new(10);
        assert!(store.should_overflow(11));
        assert!(!store.should_overflow(10));
        let h = store.append(b"hello world, large payload");
        assert_eq!(store.get(&h).unwrap(), b"hello world, large payload");
    }

    #[test]
    fn sidecar_buffer_roundtrip_with_crc() {
        let mut store = OverflowStore::new(4);
        let h1 = store.append(b"first large value");
        let h2 = store.append(b"second large value");
        let mut buf = Vec::new();
        store.flush_to_sidecar_buffer(&mut buf).unwrap();
        let mut loaded = OverflowStore::new(1024);
        loaded.load_from_bytes(&buf).unwrap();
        assert_eq!(loaded.get(&h1).unwrap(), b"first large value");
        assert_eq!(loaded.get(&h2).unwrap(), b"second large value");
        assert_eq!(loaded.index.len(), 2);
    }

    #[test]
    fn rebuild_drops_garbage() {
        let mut store = OverflowStore::new(4);
        let _ = store.append(b"dead payload here");
        store.rebuild_from_live(&[b"live payload!!".to_vec()]);
        assert_eq!(store.index.len(), 1);
        assert_eq!(
            store.get(&OverflowHandle { entry_id: 0 }).unwrap(),
            b"live payload!!"
        );
    }
}
