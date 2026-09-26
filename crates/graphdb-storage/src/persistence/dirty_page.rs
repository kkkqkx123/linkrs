//! Dirty page tracking for Shadow Page Copy-on-Write.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of rows per dirty page.
pub const ROWS_PER_PAGE: usize = 1024;

// ---------------------------------------------------------------------------
// ComponentType / PageId
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComponentType {
    VertexMeta,
    VertexColumns,
    VertexTimestamps,
    VertexIdIndexer,
    EdgeMeta,
    EdgeData,
    EdgeIndex,
}

impl ComponentType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VertexMeta => "vertex_meta",
            Self::VertexColumns => "vertex_columns",
            Self::VertexTimestamps => "vertex_timestamps",
            Self::VertexIdIndexer => "vertex_id_indexer",
            Self::EdgeMeta => "edge_meta",
            Self::EdgeData => "edge_data",
            Self::EdgeIndex => "edge_index",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageId {
    pub component: ComponentType,
    pub page_id: u64,
}

impl PageId {
    pub fn new(component: ComponentType, page_id: u64) -> Self {
        Self { component, page_id }
    }
}

// ---------------------------------------------------------------------------
// Row/page mapping
// ---------------------------------------------------------------------------

/// Map a row index to its dirty-page id.
#[inline]
pub fn row_to_page(row_idx: usize) -> usize {
    row_idx / ROWS_PER_PAGE
}

// ---------------------------------------------------------------------------
// PageHeader / PageData
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct PageHeader {
    pub page_id: u32,
    pub checksum: u32,
    pub size: u32,
    pub flags: u16,
}

impl PageHeader {
    pub const FLAG_DIRTY: u16 = 0b01;
    pub const FLAG_COMPRESSED: u16 = 0b10;
    pub const SERIALIZED_SIZE: usize = 14;

    pub fn new(page_id: u32, size: u32, is_dirty: bool, is_compressed: bool) -> Self {
        let mut flags = 0u16;
        if is_dirty {
            flags |= Self::FLAG_DIRTY;
        }
        if is_compressed {
            flags |= Self::FLAG_COMPRESSED;
        }
        Self {
            page_id,
            checksum: 0,
            size,
            flags,
        }
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SERIALIZED_SIZE {
            return None;
        }
        let page_id = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let checksum = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let size = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        let flags = u16::from_le_bytes([data[12], data[13]]);
        Some(Self {
            page_id,
            checksum,
            size,
            flags,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PageData {
    pub header: PageHeader,
    pub data: Vec<u8>,
}

impl PageData {
    pub fn new(page_id: u32, data: Vec<u8>, is_compressed: bool) -> Self {
        let checksum = crc32fast::hash(&data);
        let mut header = PageHeader::new(page_id, data.len() as u32, true, is_compressed);
        header.checksum = checksum;
        Self { header, data }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.header.page_id.to_le_bytes());
        buf.extend_from_slice(&self.header.checksum.to_le_bytes());
        buf.extend_from_slice(&self.header.size.to_le_bytes());
        buf.extend_from_slice(&self.header.flags.to_le_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < PageHeader::SERIALIZED_SIZE {
            return None;
        }
        let header = PageHeader::deserialize(&data[..PageHeader::SERIALIZED_SIZE])?;
        let payload = data[PageHeader::SERIALIZED_SIZE..].to_vec();
        // Validate the declared size matches the actual payload so a truncated
        // or corrupt record is rejected even when the checksum would be
        // recomputable from the (wrong) remainder.
        if header.size as usize != payload.len() {
            return None;
        }
        let expected = crc32fast::hash(&payload);
        if expected != header.checksum {
            return None;
        }
        Some(Self {
            header,
            data: payload,
        })
    }
}

// ---------------------------------------------------------------------------
// Checkpoint strategy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CheckpointStrategy {
    Incremental,
    Hybrid,
    #[default]
    Full,
}

impl CheckpointStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incremental => "incremental",
            Self::Hybrid => "hybrid",
            Self::Full => "full",
        }
    }
}

pub fn select_checkpoint_strategy(dirty_ratio: f64) -> CheckpointStrategy {
    if dirty_ratio < 0.1 {
        CheckpointStrategy::Incremental
    } else if dirty_ratio < 0.5 {
        CheckpointStrategy::Hybrid
    } else {
        CheckpointStrategy::Full
    }
}

// ---------------------------------------------------------------------------
// Incremental checkpoint metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalCheckpointMeta {
    pub base_checkpoint_id: Option<u64>,
    pub dirty_pages: Vec<PageId>,
    pub page_checksums: HashMap<PageId, u32>,
    pub total_pages: usize,
    pub dirty_ratio: f64,
    pub strategy: CheckpointStrategy,
}

impl Default for IncrementalCheckpointMeta {
    fn default() -> Self {
        Self {
            base_checkpoint_id: None,
            dirty_pages: Vec::new(),
            page_checksums: HashMap::new(),
            total_pages: 0,
            dirty_ratio: 0.0,
            strategy: CheckpointStrategy::Full,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_row_to_page() {
        assert_eq!(row_to_page(0), 0);
        assert_eq!(row_to_page(1023), 0);
        assert_eq!(row_to_page(1024), 1);
    }

    #[test]
    fn test_page_data_checksum() {
        let data = b"hello world".to_vec();
        let page = PageData::new(1, data.clone(), false);
        let serialized = page.serialize();
        let decoded = PageData::deserialize(&serialized).unwrap();
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn test_page_data_accepts_large_payloads() {
        let data = vec![7u8; 70_000];
        let page = PageData::new(9, data.clone(), false);
        assert_eq!(page.header.size as usize, data.len());

        let decoded = PageData::deserialize(&page.serialize()).expect("large page roundtrip");
        assert_eq!(decoded.header.size as usize, data.len());
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn test_page_data_rejects_size_mismatch() {
        let mut bytes = PageData::new(3, b"payload".to_vec(), false).serialize();
        bytes[8..12].copy_from_slice(&3u32.to_le_bytes());
        assert!(PageData::deserialize(&bytes).is_none());
    }

    #[test]
    fn test_page_data_rejects_corrupt_payload() {
        let mut bytes = PageData::new(4, b"payload".to_vec(), false).serialize();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert!(PageData::deserialize(&bytes).is_none());
    }

    #[test]
    fn test_strategy_selection() {
        assert_eq!(
            select_checkpoint_strategy(0.05),
            CheckpointStrategy::Incremental
        );
        assert_eq!(select_checkpoint_strategy(0.2), CheckpointStrategy::Hybrid);
        assert_eq!(select_checkpoint_strategy(0.7), CheckpointStrategy::Full);
    }
}
