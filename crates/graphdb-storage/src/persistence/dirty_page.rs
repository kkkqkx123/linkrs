//! Dirty page tracking for Shadow Page Copy-on-Write.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};

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
// DirtyPageTracker
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct DirtyPageTracker {
    dirty_pages: BTreeSet<u32>,
    dirty_count: AtomicUsize,
    total_pages: usize,
}

impl Clone for DirtyPageTracker {
    fn clone(&self) -> Self {
        Self {
            dirty_pages: self.dirty_pages.clone(),
            dirty_count: AtomicUsize::new(self.dirty_count.load(Ordering::Relaxed)),
            total_pages: self.total_pages,
        }
    }
}

impl DirtyPageTracker {
    pub fn new(total_pages: usize) -> Self {
        Self {
            dirty_pages: BTreeSet::new(),
            dirty_count: AtomicUsize::new(0),
            total_pages,
        }
    }

    pub fn mark_page(&mut self, page_id: usize) -> bool {
        let key = page_id as u32;
        if self.dirty_pages.insert(key) {
            self.dirty_count.fetch_add(1, Ordering::Relaxed);
            if page_id >= self.total_pages {
                self.total_pages = page_id + 1;
            }
            true
        } else {
            false
        }
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty_count.load(Ordering::Relaxed)
    }

    pub fn total_pages(&self) -> usize {
        self.total_pages
    }

    pub fn set_total_pages(&mut self, total: usize) {
        self.total_pages = total;
    }

    pub fn dirty_pages(&self) -> Vec<usize> {
        self.dirty_pages.iter().map(|&id| id as usize).collect()
    }

    pub fn clear_page(&mut self, page_id: usize) -> bool {
        let key = page_id as u32;
        if self.dirty_pages.remove(&key) {
            self.dirty_count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.dirty_pages.clear();
        self.dirty_count.store(0, Ordering::Relaxed);
    }

    pub fn ensure_rows(&mut self, num_rows: usize) {
        let needed_pages = num_rows.div_ceil(ROWS_PER_PAGE);
        if needed_pages > self.total_pages {
            self.total_pages = needed_pages;
        }
    }

    #[inline]
    pub fn row_to_page(row_idx: usize) -> usize {
        row_idx / ROWS_PER_PAGE
    }
}

impl Default for DirtyPageTracker {
    fn default() -> Self {
        Self::new(0)
    }
}

// ---------------------------------------------------------------------------
// PageHeader / PageData
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct PageHeader {
    pub page_id: u32,
    pub checksum: u32,
    pub size: u16,
    pub flags: u16,
}

impl PageHeader {
    pub const FLAG_DIRTY: u16 = 0b01;
    pub const FLAG_COMPRESSED: u16 = 0b10;
    pub const SERIALIZED_SIZE: usize = 12;

    pub fn new(page_id: u32, size: u16, is_dirty: bool, is_compressed: bool) -> Self {
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
        let size = u16::from_le_bytes([data[8], data[9]]);
        let flags = u16::from_le_bytes([data[10], data[11]]);
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
        let size = data.len().min(u16::MAX as usize) as u16;
        let checksum = crc32fast::hash(&data);
        let mut header = PageHeader::new(page_id, size, true, is_compressed);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckpointStrategy {
    Incremental,
    Hybrid,
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
    fn test_mark_and_query() {
        let mut tracker = DirtyPageTracker::new(10);
        assert_eq!(tracker.dirty_count(), 0);
        assert!(tracker.mark_page(2));
        assert!(!tracker.mark_page(2));
        assert_eq!(tracker.dirty_count(), 1);
        assert_eq!(tracker.dirty_pages(), vec![2]);
    }

    #[test]
    fn test_clear() {
        let mut tracker = DirtyPageTracker::new(10);
        tracker.mark_page(5);
        tracker.clear();
        assert_eq!(tracker.dirty_count(), 0);
    }

    #[test]
    fn test_collect() {
        let mut tracker = DirtyPageTracker::new(10);
        tracker.mark_page(1);
        tracker.mark_page(3);
        let pages = tracker.dirty_pages();
        assert_eq!(pages, vec![1, 3]);
    }

    #[test]
    fn test_row_to_page() {
        assert_eq!(DirtyPageTracker::row_to_page(0), 0);
        assert_eq!(DirtyPageTracker::row_to_page(1023), 0);
        assert_eq!(DirtyPageTracker::row_to_page(1024), 1);
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
    fn test_strategy_selection() {
        assert_eq!(
            select_checkpoint_strategy(0.05),
            CheckpointStrategy::Incremental
        );
        assert_eq!(select_checkpoint_strategy(0.2), CheckpointStrategy::Hybrid);
        assert_eq!(select_checkpoint_strategy(0.7), CheckpointStrategy::Full);
    }
}
