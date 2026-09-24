//! Chunk residency tracking and eviction snapshots.
//!
//! `ChunkResidency` tracks whether a chunk's decoded data is in memory
//! (`Resident`) or released with only a compressed snapshot retained
//! (`Evicted`). Evicted chunks keep their row range, pre-evict encoding
//! scheme, compression profile, and column-level statistics: only the
//! decoded buffers and encoding structures are freed.
//!
//! The snapshot reuses the dirty-page envelope ([`PageData`]) and its
//! row-page addressing, so no new file or page type is introduced. Each
//! snapshot page is individually checksummed and compressed, giving
//! bounded-cost point reads (one page) and one-shot batch reloads.

use std::sync::atomic::{AtomicU64, Ordering};

use graphdb_core::{StorageError, StorageResult, Value};

use crate::encoding::{ChunkEncodingMeta, EncodingType};
use crate::persistence::dirty_page::{PageData, ROWS_PER_PAGE};

/// zstd level for eviction snapshots. Eviction runs off the hot path
/// (watermark-triggered background work), so this favors ratio over speed.
pub const EVICT_ZSTD_LEVEL: i32 = 3;

/// Monotonic access clock shared by all chunks. Point reads stamp their
/// chunk on the shared reference path (atomics, no lock upgrade), and the
/// watermark-triggered eviction pass releases the oldest chunks first.
static EVICT_CLOCK: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_tick() -> u64 {
    EVICT_CLOCK.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// ChunkResidency
// ---------------------------------------------------------------------------

/// Memory residency state of a [`ColumnChunk`](super::chunk::ColumnChunk).
#[derive(Debug, Clone, Default)]
pub enum ChunkResidency {
    /// Decoded data is in memory and accessible.
    #[default]
    Resident,
    /// Decoded buffers released; compressed snapshot retained for
    /// on-demand reload. Row range, encoding scheme, and profiles stay.
    Evicted(EvictedSnapshot),
}

impl ChunkResidency {
    /// Whether decoded data is in memory.
    pub fn is_resident(&self) -> bool {
        matches!(self, Self::Resident)
    }

    /// Whether only the compressed snapshot is retained.
    pub fn is_evicted(&self) -> bool {
        matches!(self, Self::Evicted(_))
    }

    /// Borrow the eviction snapshot, if evicted.
    pub fn evicted_snapshot(&self) -> Option<&EvictedSnapshot> {
        match self {
            Self::Evicted(snapshot) => Some(snapshot),
            Self::Resident => None,
        }
    }

    /// Effective encoding scheme: live encoding when resident, pre-evict
    /// scheme when evicted (for metadata and flush sidecars).
    pub fn effective_encoding(&self, live: EncodingType) -> EncodingType {
        match self {
            Self::Resident => live,
            Self::Evicted(snapshot) => snapshot.encoding,
        }
    }
}

// ---------------------------------------------------------------------------
// EvictedSnapshot
// ---------------------------------------------------------------------------

/// Compressed snapshot of one evicted chunk's current values.
///
/// Values are captured overlay-merged (what `get` serves), split along the
/// shared row-page addressing, and stored per page as a checksummed
/// [`PageData`] envelope compressed with zstd. Point reads decode a single
/// page; batch reloads decode each page once.
#[derive(Debug, Clone)]
pub struct EvictedSnapshot {
    /// Rows covered, starting at the chunk's `row_offset`.
    pub rows: u32,
    /// Pre-evict encoding scheme, retained for chunk metadata.
    pub encoding: EncodingType,
    /// Pre-evict compression profile, restored on promotion.
    pub meta: ChunkEncodingMeta,
    /// Compressed pages in row order.
    pub pages: Vec<EvictedPage>,
    /// Uncompressed envelope bytes (for memory accounting).
    pub uncompressed_bytes: usize,
}

/// One compressed row page inside an eviction snapshot.
#[derive(Debug, Clone)]
pub struct EvictedPage {
    /// Absolute row-page id (`row / ROWS_PER_PAGE`), shared with the
    /// dirty-page and incremental-checkpoint addressing.
    pub page_id: u32,
    /// First absolute row covered by this page.
    pub start_row: u32,
    /// Rows covered by this page.
    pub rows: u32,
    /// zstd-compressed [`PageData`] serialization (checksum envelope).
    pub compressed: Vec<u8>,
}

impl EvictedSnapshot {
    /// Capture chunk values starting at absolute `start_row` with the
    /// pre-evict `encoding` scheme tag and compression profile.
    pub fn capture(
        start_row: usize,
        values: Vec<Option<Value>>,
        encoding: EncodingType,
        meta: ChunkEncodingMeta,
    ) -> StorageResult<Self> {
        let rows = values.len() as u32;
        let mut pages = Vec::new();
        let mut uncompressed_bytes = 0usize;
        for (page_no, window) in values.chunks(ROWS_PER_PAGE).enumerate() {
            let abs_start = start_row + page_no * ROWS_PER_PAGE;
            let payload = postcard::to_allocvec(window)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            let page = PageData::new((abs_start / ROWS_PER_PAGE) as u32, payload, false);
            let serialized = page.serialize();
            uncompressed_bytes += serialized.len();
            let compressed =
                zstd::encode_all(serialized.as_slice(), EVICT_ZSTD_LEVEL).map_err(|e| {
                    StorageError::io_error(format!("eviction snapshot compress failed: {}", e))
                })?;
            pages.push(EvictedPage {
                page_id: (abs_start / ROWS_PER_PAGE) as u32,
                start_row: abs_start as u32,
                rows: window.len() as u32,
                compressed,
            });
        }
        Ok(Self {
            rows,
            encoding,
            meta,
            pages,
            uncompressed_bytes,
        })
    }

    /// Compressed bytes retained (resident-memory replacement cost).
    pub fn compressed_bytes(&self) -> usize {
        self.pages.iter().map(|p| p.compressed.len()).sum()
    }

    /// Decode one absolute row. Used by the shared-reference read path;
    /// failures are reported so `&mut` callers can propagate them.
    pub fn decode_row(&self, abs_row: usize) -> StorageResult<Option<Value>> {
        let page = self.page_for(abs_row).ok_or_else(|| {
            StorageError::deserialize_error(format!(
                "evicted snapshot has no page for row {}",
                abs_row
            ))
        })?;
        let values = Self::decode_page(page)?;
        let offset = abs_row - page.start_row as usize;
        values.get(offset).cloned().ok_or_else(|| {
            StorageError::deserialize_error(format!(
                "evicted snapshot page {} missing row {}",
                page.page_id, abs_row
            ))
        })
    }

    /// Decode every covered row as `(absolute row, value)` pairs in order.
    /// Used by batch reloads so each page is decompressed exactly once.
    pub fn decode_all(&self) -> StorageResult<Vec<(usize, Option<Value>)>> {
        let mut out = Vec::with_capacity(self.rows as usize);
        for page in &self.pages {
            let values = Self::decode_page(page)?;
            for (offset, value) in values.into_iter().enumerate() {
                out.push((page.start_row as usize + offset, value));
            }
        }
        Ok(out)
    }

    fn page_for(&self, abs_row: usize) -> Option<&EvictedPage> {
        self.pages.iter().find(|p| {
            let start = p.start_row as usize;
            abs_row >= start && abs_row < start + p.rows as usize
        })
    }

    fn decode_page(page: &EvictedPage) -> StorageResult<Vec<Option<Value>>> {
        let decompressed = zstd::decode_all(page.compressed.as_slice()).map_err(|e| {
            StorageError::deserialize_error(format!(
                "evicted snapshot page {} decompress failed at row {}: {}",
                page.page_id, page.start_row, e
            ))
        })?;
        let envelope = PageData::deserialize(&decompressed).ok_or_else(|| {
            StorageError::deserialize_error(format!(
                "evicted snapshot page {} checksum mismatch at row {}",
                page.page_id, page.start_row
            ))
        })?;
        postcard::from_bytes(&envelope.data).map_err(|e| {
            StorageError::deserialize_error(format!(
                "evicted snapshot page {} payload corrupt at row {}: {}",
                page.page_id, page.start_row, e
            ))
        })
    }
}
