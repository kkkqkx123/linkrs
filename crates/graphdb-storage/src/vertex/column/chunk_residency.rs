//! Chunk residency tracking and eviction snapshots.
//!
//! `ChunkResidency` tracks whether a chunk's decoded data is in memory
//! (`Resident`) or released (`Evicted`). Evicted chunks keep their row
//! range, pre-evict encoding scheme, compression profile, and column-level
//! statistics; the decoded buffers are freed and the compressed snapshot
//! pages spill to per-snapshot files under the process spill directory, so
//! eviction genuinely releases heap memory instead of retaining it.
//!
//! The snapshot reuses the dirty-page envelope ([`PageData`]) and its
//! row-page addressing, so no new file or page type is introduced. Each
//! snapshot page is individually checksummed and compressed, giving
//! bounded-cost point reads (one page) and one-shot batch reloads. Spill
//! files are reference-counted: clones share one file and the last dropped
//! reference deletes it. A spill write failure fails the eviction, never
//! silently keeping the snapshot in memory.
//!
//! Checkpoint sidecars persist evicted snapshots across restarts: full flush
//! writes one `{column}.snapshot` file per column holding the already
//! compressed pages of every evicted chunk, and reload re-evicts matching
//! chunks backed by a read-only `mmap` of the sidecar instead of decoding
//! them onto the heap. Sidecars are derived caches excluded from the commit
//! manifest: a missing or corrupt sidecar only keeps chunks resident and
//! never fails the open.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
// Spill directory
// ---------------------------------------------------------------------------

/// Sequence numbering spill files within the process.
static SPILL_SEQ: AtomicU64 = AtomicU64::new(1);

/// Process spill directory holding eviction spill files. Scoped by process
/// id so concurrent processes never share files; unique sequence numbers
/// keep files distinct within the process.
fn vertex_spill_dir() -> StorageResult<PathBuf> {
    let dir = std::env::temp_dir().join(format!("graphdb-vertex-spill-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| {
        StorageError::io_error(format!(
            "vertex spill directory {} unavailable: {}",
            dir.display(),
            e
        ))
    })?;
    Ok(dir)
}

/// Remove crash-orphaned spill directories from previous processes.
/// Current process directory is kept. Tolerant by design: cleanup failures
/// only warn and never fail startup.
pub fn cleanup_stale_spill_dirs() {
    let tmp = std::env::temp_dir();
    let current = format!("graphdb-vertex-spill-{}", std::process::id());
    let Ok(entries) = std::fs::read_dir(&tmp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("graphdb-vertex-spill-") || name == current {
            continue;
        }
        let path = entry.path();
        let still_running = name
            .strip_prefix("graphdb-vertex-spill-")
            .and_then(|pid| pid.parse::<u32>().ok())
            .is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
        if still_running {
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(&path) {
            log::warn!(
                "vertex spill cleanup: cannot remove {}: {}",
                path.display(),
                e
            );
        }
    }
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
///
/// In-memory snapshots keep the pages on the heap; spilled snapshots keep
/// them in a reference-counted spill file and only page metadata in memory.
/// Clones share one spill file and the last dropped clone deletes it.
#[derive(Debug, Clone)]
pub struct EvictedSnapshot {
    /// Rows covered, starting at the chunk's `row_offset`.
    pub rows: u32,
    /// Pre-evict encoding scheme, retained for chunk metadata.
    pub encoding: EncodingType,
    /// Pre-evict compression profile, restored on promotion.
    pub meta: ChunkEncodingMeta,
    /// Snapshot pages, heap or spill-file resident.
    pages: SnapshotPages,
    /// Uncompressed envelope bytes (for memory accounting).
    pub uncompressed_bytes: usize,
}

/// Where an evicted snapshot's compressed pages live.
#[derive(Debug, Clone)]
enum SnapshotPages {
    /// Compressed pages retained on the heap.
    Memory(Vec<EvictedPage>),
    /// Compressed pages in a spill file, shared across clones.
    Spilled(Arc<SpilledPages>),
    /// Compressed pages in a checkpoint sidecar, shared across clones.
    /// The mapping is read-only page cache; only frame metadata counts
    /// toward heap usage.
    Mapped(Arc<MappedSnapshotPages>),
}

/// Reference-counted spill file for one evicted chunk. Dropping the last
/// reference deletes the file; partial files from failed spills are removed
/// by the spilling call itself.
#[derive(Debug)]
struct SpilledPages {
    path: PathBuf,
    frames: Vec<SpilledFrame>,
}

/// One framed page inside a spill file: `len:u32` prefix followed by the
/// compressed [`PageData`] serialization (checksum envelope).
#[derive(Debug, Clone, Copy)]
struct SpilledFrame {
    /// Absolute row-page id (`row / ROWS_PER_PAGE`), shared with the
    /// dirty-page and incremental-checkpoint addressing.
    page_id: u32,
    /// First absolute row covered by this page.
    start_row: u32,
    /// Rows covered by this page.
    rows: u32,
    /// Byte offset of the frame (length prefix) in the spill file.
    offset: u64,
    /// Compressed payload bytes excluding the length prefix.
    len: u32,
}

impl Drop for SpilledPages {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
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

/// One framed page inside a memory-mapped checkpoint sidecar.
///
/// Byte layout mirrors [`SpilledFrame`] (`len:u32` prefix plus the
/// compressed [`PageData`] serialization), addressed as slices of the
/// shared mapping instead of file offset reads.
#[derive(Debug, Clone, Copy)]
pub struct MappedFrame {
    /// Absolute row-page id (`row / ROWS_PER_PAGE`).
    pub page_id: u32,
    /// First absolute row covered by this page.
    pub start_row: u32,
    /// Rows covered by this page.
    pub rows: u32,
    /// Byte offset of the frame (length prefix) in the mapping.
    pub offset: u64,
    /// Compressed payload bytes excluding the length prefix.
    pub len: u32,
}

/// Reference-counted view of checkpoint sidecar pages. Clones share one
/// mapping; the sidecar file itself is owned by the checkpoint directory
/// and never deleted by readers.
#[derive(Debug, Clone)]
pub struct MappedSnapshotPages {
    map: Arc<memmap2::Mmap>,
    frames: Vec<MappedFrame>,
}

impl EvictedSnapshot {
    /// Capture chunk values starting at absolute `start_row` with the
    /// pre-evict `encoding` scheme tag and compression profile. Pages stay
    /// on the heap; eviction prefers [`Self::capture_spilled`].
    pub fn capture(
        start_row: usize,
        values: Vec<Option<Value>>,
        encoding: EncodingType,
        meta: ChunkEncodingMeta,
    ) -> StorageResult<Self> {
        let rows = values.len() as u32;
        let compressed = Self::compress_pages(start_row, &values)?;
        let mut pages = Vec::with_capacity(compressed.len());
        let mut uncompressed_bytes = 0usize;
        for (page_no, (payload, serialized_len)) in compressed.into_iter().enumerate() {
            let abs_start = start_row + page_no * ROWS_PER_PAGE;
            let window_rows = values
                .len()
                .saturating_sub(page_no * ROWS_PER_PAGE)
                .min(ROWS_PER_PAGE) as u32;
            uncompressed_bytes += serialized_len;
            pages.push(EvictedPage {
                page_id: (abs_start / ROWS_PER_PAGE) as u32,
                start_row: abs_start as u32,
                rows: window_rows,
                compressed: payload,
            });
        }
        Ok(Self {
            rows,
            encoding,
            meta,
            pages: SnapshotPages::Memory(pages),
            uncompressed_bytes,
        })
    }

    /// Capture chunk values and spill the compressed pages to a spill file,
    /// keeping only page metadata in memory. A spill failure removes the
    /// partial file and reports an error so the caller skips the eviction
    /// instead of silently retaining heap pages.
    pub fn capture_spilled(
        start_row: usize,
        values: Vec<Option<Value>>,
        encoding: EncodingType,
        meta: ChunkEncodingMeta,
    ) -> StorageResult<Self> {
        let rows = values.len() as u32;
        let compressed = Self::compress_pages(start_row, &values)?;
        let mut uncompressed_bytes = 0usize;
        for (_, serialized_len) in &compressed {
            uncompressed_bytes += serialized_len;
        }
        let dir = vertex_spill_dir()?;
        let seq = SPILL_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("spill-{}-{}.pages", std::process::id(), seq));
        let spill_result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| {
                    StorageError::io_error(format!(
                        "vertex spill file {} unavailable: {}",
                        path.display(),
                        e
                    ))
                })?;
            let mut frames = Vec::with_capacity(compressed.len());
            let mut offset = 0u64;
            for (page_no, (payload, _)) in compressed.into_iter().enumerate() {
                let abs_start = start_row + page_no * ROWS_PER_PAGE;
                let window_rows = (rows as usize)
                    .saturating_sub(page_no * ROWS_PER_PAGE)
                    .min(ROWS_PER_PAGE) as u32;
                file.write_all(&(payload.len() as u32).to_le_bytes())
                    .map_err(|e| {
                        StorageError::io_error(format!(
                            "vertex spill write to {} failed: {}",
                            path.display(),
                            e
                        ))
                    })?;
                file.write_all(&payload).map_err(|e| {
                    StorageError::io_error(format!(
                        "vertex spill write to {} failed: {}",
                        path.display(),
                        e
                    ))
                })?;
                frames.push(SpilledFrame {
                    page_id: (abs_start / ROWS_PER_PAGE) as u32,
                    start_row: abs_start as u32,
                    rows: window_rows,
                    offset,
                    len: payload.len() as u32,
                });
                offset += 4 + payload.len() as u64;
            }
            Ok::<Vec<SpilledFrame>, StorageError>(frames)
        })();
        match spill_result {
            Ok(frames) => Ok(Self {
                rows,
                encoding,
                meta,
                pages: SnapshotPages::Spilled(Arc::new(SpilledPages { path, frames })),
                uncompressed_bytes,
            }),
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        }
    }

    /// Compress one value window per row page. Returns the compressed
    /// payloads with their uncompressed envelope sizes.
    fn compress_pages(
        start_row: usize,
        values: &[Option<Value>],
    ) -> StorageResult<Vec<(Vec<u8>, usize)>> {
        let mut out = Vec::new();
        for (page_no, window) in values.chunks(ROWS_PER_PAGE).enumerate() {
            let abs_start = start_row + page_no * ROWS_PER_PAGE;
            let payload = postcard::to_allocvec(window)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            let page = PageData::new((abs_start / ROWS_PER_PAGE) as u32, payload, false);
            let serialized = page.serialize();
            let serialized_len = serialized.len();
            let compressed =
                zstd::encode_all(serialized.as_slice(), EVICT_ZSTD_LEVEL).map_err(|e| {
                    StorageError::io_error(format!("eviction snapshot compress failed: {}", e))
                })?;
            out.push((compressed, serialized_len));
        }
        Ok(out)
    }

    /// Compressed bytes of the snapshot payload, wherever it lives
    /// (heap, spill file, or checkpoint mapping). Used for eviction
    /// observability.
    pub fn compressed_bytes(&self) -> usize {
        match &self.pages {
            SnapshotPages::Memory(pages) => pages.iter().map(|p| p.compressed.len()).sum(),
            SnapshotPages::Spilled(spilled) => spilled.frames.iter().map(|f| f.len as usize).sum(),
            SnapshotPages::Mapped(mapped) => mapped.frames.iter().map(|f| f.len as usize).sum(),
        }
    }

    /// Heap bytes retained by the snapshot: the full payload for
    /// in-memory snapshots, only page metadata for spilled or mapped ones.
    pub fn resident_bytes(&self) -> usize {
        match &self.pages {
            SnapshotPages::Memory(pages) => pages.iter().map(|p| p.compressed.len()).sum(),
            SnapshotPages::Spilled(_) | SnapshotPages::Mapped(_) => 0,
        }
    }

    /// Whether the snapshot pages spilled to a file.
    pub fn is_spilled(&self) -> bool {
        matches!(self.pages, SnapshotPages::Spilled(_))
    }

    /// Whether the snapshot pages map a checkpoint sidecar.
    pub fn is_mapped(&self) -> bool {
        matches!(self.pages, SnapshotPages::Mapped(_))
    }

    /// Build an evicted snapshot over checkpoint sidecar pages without
    /// copying payloads. Used by reload to re-evict chunks whose pages
    /// already persist in the sidecar.
    pub fn from_mapped(
        rows: u32,
        encoding: EncodingType,
        meta: ChunkEncodingMeta,
        uncompressed_bytes: usize,
        map: Arc<memmap2::Mmap>,
        frames: Vec<MappedFrame>,
    ) -> Self {
        Self {
            rows,
            encoding,
            meta,
            pages: SnapshotPages::Mapped(Arc::new(MappedSnapshotPages { map, frames })),
            uncompressed_bytes,
        }
    }

    /// Materialize the compressed page payloads without decoding them, in
    /// page order. Used by checkpoint sidecar persistence so evicted chunks
    /// persist without promoting back onto the heap.
    pub fn compressed_pages(&self) -> StorageResult<Vec<EvictedPage>> {
        match &self.pages {
            SnapshotPages::Memory(pages) => Ok(pages.clone()),
            SnapshotPages::Spilled(spilled) => {
                let mut file = File::open(&spilled.path).map_err(|e| {
                    StorageError::io_error(format!(
                        "vertex spill file {} unavailable: {}",
                        spilled.path.display(),
                        e
                    ))
                })?;
                let mut out = Vec::with_capacity(spilled.frames.len());
                for frame in &spilled.frames {
                    let compressed = Self::read_frame_at(&mut file, frame)?;
                    out.push(EvictedPage {
                        page_id: frame.page_id,
                        start_row: frame.start_row,
                        rows: frame.rows,
                        compressed,
                    });
                }
                Ok(out)
            }
            SnapshotPages::Mapped(mapped) => {
                let mut out = Vec::with_capacity(mapped.frames.len());
                for frame in &mapped.frames {
                    out.push(EvictedPage {
                        page_id: frame.page_id,
                        start_row: frame.start_row,
                        rows: frame.rows,
                        compressed: Self::mapped_frame_bytes(&mapped.map, frame)?.to_vec(),
                    });
                }
                Ok(out)
            }
        }
    }

    /// Spill file path, if spilled. Test observability only.
    #[cfg(test)]
    pub(crate) fn spill_file_path(&self) -> Option<std::path::PathBuf> {
        match &self.pages {
            SnapshotPages::Spilled(spilled) => Some(spilled.path.clone()),
            SnapshotPages::Memory(_) | SnapshotPages::Mapped(_) => None,
        }
    }

    /// Decode one absolute row. Used by the shared-reference read path;
    /// failures are reported so `&mut` callers can propagate them.
    pub fn decode_row(&self, abs_row: usize) -> StorageResult<Option<Value>> {
        let (page_id, start_row, compressed) = match &self.pages {
            SnapshotPages::Memory(pages) => {
                let page = pages
                    .iter()
                    .find(|p| {
                        let start = p.start_row as usize;
                        abs_row >= start && abs_row < start + p.rows as usize
                    })
                    .ok_or_else(|| {
                        StorageError::deserialize_error(format!(
                            "evicted snapshot has no page for row {}",
                            abs_row
                        ))
                    })?;
                (page.page_id, page.start_row, page.compressed.clone())
            }
            SnapshotPages::Spilled(spilled) => {
                let frame = Self::spilled_frame_for(spilled, abs_row)?;
                (
                    frame.page_id,
                    frame.start_row,
                    Self::read_frame_bytes(spilled, &frame)?,
                )
            }
            SnapshotPages::Mapped(mapped) => {
                let frame = Self::mapped_frame_for(mapped, abs_row)?;
                (
                    frame.page_id,
                    frame.start_row,
                    Self::mapped_frame_bytes(&mapped.map, &frame)?.to_vec(),
                )
            }
        };
        let values = Self::decode_compressed(&compressed, page_id, start_row)?;
        let offset = abs_row - start_row as usize;
        values.get(offset).cloned().ok_or_else(|| {
            StorageError::deserialize_error(format!(
                "evicted snapshot page {} missing row {}",
                page_id, abs_row
            ))
        })
    }

    /// Decode every covered row as `(absolute row, value)` pairs in order.
    /// Used by batch reloads so each page is decompressed exactly once.
    pub fn decode_all(&self) -> StorageResult<Vec<(usize, Option<Value>)>> {
        let mut out = Vec::with_capacity(self.rows as usize);
        match &self.pages {
            SnapshotPages::Memory(pages) => {
                for page in pages {
                    let values =
                        Self::decode_compressed(&page.compressed, page.page_id, page.start_row)?;
                    for (offset, value) in values.into_iter().enumerate() {
                        out.push((page.start_row as usize + offset, value));
                    }
                }
            }
            SnapshotPages::Spilled(spilled) => {
                let mut file = File::open(&spilled.path).map_err(|e| {
                    StorageError::io_error(format!(
                        "vertex spill file {} unavailable: {}",
                        spilled.path.display(),
                        e
                    ))
                })?;
                for frame in &spilled.frames {
                    let compressed = Self::read_frame_at(&mut file, frame)?;
                    let values =
                        Self::decode_compressed(&compressed, frame.page_id, frame.start_row)?;
                    for (offset, value) in values.into_iter().enumerate() {
                        out.push((frame.start_row as usize + offset, value));
                    }
                }
            }
            SnapshotPages::Mapped(mapped) => {
                for frame in &mapped.frames {
                    let compressed = Self::mapped_frame_bytes(&mapped.map, frame)?;
                    let values =
                        Self::decode_compressed(compressed, frame.page_id, frame.start_row)?;
                    for (offset, value) in values.into_iter().enumerate() {
                        out.push((frame.start_row as usize + offset, value));
                    }
                }
            }
        }
        Ok(out)
    }

    fn spilled_frame_for(spilled: &SpilledPages, abs_row: usize) -> StorageResult<SpilledFrame> {
        spilled
            .frames
            .iter()
            .find(|f| {
                let start = f.start_row as usize;
                abs_row >= start && abs_row < start + f.rows as usize
            })
            .copied()
            .ok_or_else(|| {
                StorageError::deserialize_error(format!(
                    "evicted snapshot has no page for row {}",
                    abs_row
                ))
            })
    }

    fn read_frame_bytes(spilled: &SpilledPages, frame: &SpilledFrame) -> StorageResult<Vec<u8>> {
        let mut file = File::open(&spilled.path).map_err(|e| {
            StorageError::io_error(format!(
                "vertex spill file {} unavailable: {}",
                spilled.path.display(),
                e
            ))
        })?;
        Self::read_frame_at(&mut file, frame)
    }

    fn read_frame_at(file: &mut File, frame: &SpilledFrame) -> StorageResult<Vec<u8>> {
        file.seek(SeekFrom::Start(frame.offset + 4))
            .map_err(|e| StorageError::io_error(format!("vertex spill seek failed: {}", e)))?;
        let mut buf = vec![0u8; frame.len as usize];
        file.read_exact(&mut buf)
            .map_err(|e| StorageError::io_error(format!("vertex spill read failed: {}", e)))?;
        Ok(buf)
    }

    fn mapped_frame_for(
        mapped: &MappedSnapshotPages,
        abs_row: usize,
    ) -> StorageResult<MappedFrame> {
        mapped
            .frames
            .iter()
            .find(|f| {
                let start = f.start_row as usize;
                abs_row >= start && abs_row < start + f.rows as usize
            })
            .copied()
            .ok_or_else(|| {
                StorageError::deserialize_error(format!(
                    "mapped snapshot has no page for row {}",
                    abs_row
                ))
            })
    }

    fn mapped_frame_bytes<'a>(
        map: &'a memmap2::Mmap,
        frame: &MappedFrame,
    ) -> StorageResult<&'a [u8]> {
        let start = frame.offset.saturating_add(4);
        let end = start.saturating_add(frame.len as u64);
        map.get(start as usize..end as usize).ok_or_else(|| {
            StorageError::deserialize_error(format!(
                "mapped snapshot frame for rows {}..{} out of range",
                frame.start_row,
                frame.start_row as usize + frame.rows as usize,
            ))
        })
    }

    fn decode_compressed(
        compressed: &[u8],
        page_id: u32,
        start_row: u32,
    ) -> StorageResult<Vec<Option<Value>>> {
        let decompressed = zstd::decode_all(compressed).map_err(|e| {
            StorageError::deserialize_error(format!(
                "evicted snapshot page {} decompress failed at row {}: {}",
                page_id, start_row, e
            ))
        })?;
        let envelope = PageData::deserialize(&decompressed).ok_or_else(|| {
            StorageError::deserialize_error(format!(
                "evicted snapshot page {} checksum mismatch at row {}",
                page_id, start_row
            ))
        })?;
        postcard::from_bytes(&envelope.data).map_err(|e| {
            StorageError::deserialize_error(format!(
                "evicted snapshot page {} payload corrupt at row {}: {}",
                page_id, start_row, e
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// Checkpoint sidecars
// ---------------------------------------------------------------------------

/// Magic prefix of `{column}.snapshot` checkpoint sidecars (`VKSP`).
pub const VERTEX_SNAPSHOT_MAGIC: u32 = u32::from_le_bytes([0x56, 0x4B, 0x53, 0x50]);

/// One evicted chunk's pages staged for sidecar encoding. Payloads come
/// from [`EvictedSnapshot::compressed_pages`], so persisting never
/// promotes the live chunk back onto the heap.
pub struct SnapshotChunkPlan<'a> {
    pub row_offset: u32,
    pub rows: u32,
    pub encoding: EncodingType,
    pub meta: &'a ChunkEncodingMeta,
    pub uncompressed_bytes: u64,
    pub pages: Vec<EvictedPage>,
}

/// One evicted chunk restored from a sidecar mapping.
#[derive(Debug, Clone)]
pub struct MappedChunk {
    pub row_offset: u32,
    pub rows: u32,
    pub encoding: EncodingType,
    pub meta: ChunkEncodingMeta,
    pub uncompressed_bytes: usize,
    pub frames: Vec<MappedFrame>,
}

/// A memory-mapped checkpoint sidecar: the shared mapping plus one record
/// per persisted evicted chunk.
#[derive(Debug, Clone)]
pub struct MappedSnapshot {
    pub map: Arc<memmap2::Mmap>,
    pub chunks: Vec<MappedChunk>,
}

fn snapshot_error(message: String) -> StorageError {
    StorageError::deserialize_error(format!("vertex snapshot sidecar corrupt: {}", message))
}

/// Encode evicted chunks into sidecar bytes.
///
/// Layout: `magic u32, chunk_count u32`, then per chunk `row_offset u32,
/// rows u32, encoding u8, meta_len u32, meta, uncompressed u64,
/// frame_count u32` plus per frame `page_id u32, start_row u32, rows u32,
/// offset u64, len u32` (offsets relative to the payload region), then the
/// payload region (`len u32` prefix plus compressed bytes per frame), then
/// `crc32 u32` over all preceding bytes. The caller persists atomically;
/// readers validate every field before trusting a slice.
pub fn encode_snapshot_sidecar(chunks: &[SnapshotChunkPlan<'_>]) -> StorageResult<Vec<u8>> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&VERTEX_SNAPSHOT_MAGIC.to_le_bytes());
    buf.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    let mut payloads = Vec::new();
    for chunk in chunks {
        buf.extend_from_slice(&chunk.row_offset.to_le_bytes());
        buf.extend_from_slice(&chunk.rows.to_le_bytes());
        buf.push(chunk.encoding.to_u8());
        let mut meta_buf = Vec::new();
        chunk
            .meta
            .serialize(&mut meta_buf)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        buf.extend_from_slice(&(meta_buf.len() as u32).to_le_bytes());
        buf.extend_from_slice(&meta_buf);
        buf.extend_from_slice(&chunk.uncompressed_bytes.to_le_bytes());
        buf.extend_from_slice(&(chunk.pages.len() as u32).to_le_bytes());
        let mut offset = payloads.len() as u64;
        for page in &chunk.pages {
            buf.extend_from_slice(&page.page_id.to_le_bytes());
            buf.extend_from_slice(&page.start_row.to_le_bytes());
            buf.extend_from_slice(&page.rows.to_le_bytes());
            buf.extend_from_slice(&offset.to_le_bytes());
            buf.extend_from_slice(&(page.compressed.len() as u32).to_le_bytes());
            payloads.extend_from_slice(&(page.compressed.len() as u32).to_le_bytes());
            payloads.extend_from_slice(&page.compressed);
            offset += 4 + page.compressed.len() as u64;
        }
    }
    buf.extend_from_slice(&payloads);
    let checksum = crc32fast::hash(&buf);
    buf.extend_from_slice(&checksum.to_le_bytes());
    Ok(buf)
}

/// Open and validate a checkpoint sidecar. Any structural problem is an
/// error; callers keep the affected chunks resident and never fail the
/// open over a derived cache.
pub fn open_snapshot_sidecar(path: &Path) -> StorageResult<MappedSnapshot> {
    let file = File::open(path).map_err(|e| {
        StorageError::io_error(format!("vertex snapshot sidecar open failed: {}", e))
    })?;
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| {
        StorageError::io_error(format!("vertex snapshot sidecar map failed: {}", e))
    })?;
    parse_snapshot_sidecar(Arc::new(map))
}

fn parse_snapshot_sidecar(map: Arc<memmap2::Mmap>) -> StorageResult<MappedSnapshot> {
    use std::convert::TryInto;

    let bytes: &[u8] = &map;
    if bytes.len() < 12 {
        return Err(snapshot_error(format!(
            "truncated header in {} bytes",
            bytes.len()
        )));
    }
    let stored = u32::from_le_bytes(
        bytes[bytes.len() - 4..]
            .try_into()
            .map_err(|_| snapshot_error("missing checksum tail".to_string()))?,
    );
    if crc32fast::hash(&bytes[..bytes.len() - 4]) != stored {
        return Err(snapshot_error("checksum mismatch".to_string()));
    }
    let mut cursor = &bytes[..bytes.len() - 4];
    let take = |cursor: &mut &[u8], len: usize, field: &str| -> StorageResult<Vec<u8>> {
        if len > cursor.len() {
            return Err(snapshot_error(format!(
                "{} length {} exceeds remaining {}",
                field,
                len,
                cursor.len()
            )));
        }
        let (head, tail) = cursor.split_at(len);
        *cursor = tail;
        Ok(head.to_vec())
    };
    let magic = u32::from_le_bytes(
        take(&mut cursor, 4, "magic")?[..4]
            .try_into()
            .map_err(|_| snapshot_error("magic malformed".to_string()))?,
    );
    if magic != VERTEX_SNAPSHOT_MAGIC {
        return Err(snapshot_error(format!("unexpected magic {:#010x}", magic)));
    }
    let chunk_count = u32::from_le_bytes(
        take(&mut cursor, 4, "chunk_count")?[..4]
            .try_into()
            .map_err(|_| snapshot_error("chunk count malformed".to_string()))?,
    ) as usize;
    struct RawFrame {
        page_id: u32,
        start_row: u32,
        rows: u32,
        offset: u64,
        len: u32,
    }
    struct RawChunk {
        row_offset: u32,
        rows: u32,
        encoding: EncodingType,
        meta: ChunkEncodingMeta,
        uncompressed_bytes: usize,
        frames: Vec<RawFrame>,
    }
    let mut raw_chunks = Vec::with_capacity(chunk_count.min(1 << 16));
    for _ in 0..chunk_count {
        let row_offset = u32::from_le_bytes(
            take(&mut cursor, 4, "row_offset")?[..4]
                .try_into()
                .map_err(|_| snapshot_error("row offset malformed".to_string()))?,
        );
        let rows = u32::from_le_bytes(
            take(&mut cursor, 4, "rows")?[..4]
                .try_into()
                .map_err(|_| snapshot_error("rows malformed".to_string()))?,
        );
        let enc_byte = take(&mut cursor, 1, "encoding")?[0];
        if enc_byte > 6 {
            return Err(snapshot_error(format!("unknown encoding {}", enc_byte)));
        }
        let encoding = EncodingType::from_u8(enc_byte);
        let meta_len = u32::from_le_bytes(
            take(&mut cursor, 4, "meta_len")?[..4]
                .try_into()
                .map_err(|_| snapshot_error("meta length malformed".to_string()))?,
        ) as usize;
        let meta_bytes = take(&mut cursor, meta_len, "meta")?;
        let meta = if meta_bytes.is_empty() {
            ChunkEncodingMeta::default()
        } else {
            ChunkEncodingMeta::deserialize(&mut &meta_bytes[..])
                .map_err(|e| snapshot_error(format!("chunk meta corrupt: {}", e)))?
        };
        let mut uncompressed = [0u8; 8];
        uncompressed.copy_from_slice(&take(&mut cursor, 8, "uncompressed")?);
        let frame_count = u32::from_le_bytes(
            take(&mut cursor, 4, "frame_count")?[..4]
                .try_into()
                .map_err(|_| snapshot_error("frame count malformed".to_string()))?,
        ) as usize;
        let mut frames = Vec::with_capacity(frame_count.min(1 << 16));
        for _ in 0..frame_count {
            let mut u32b = [0u8; 4];
            u32b.copy_from_slice(&take(&mut cursor, 4, "page_id")?);
            let page_id = u32::from_le_bytes(u32b);
            u32b.copy_from_slice(&take(&mut cursor, 4, "start_row")?);
            let start_row = u32::from_le_bytes(u32b);
            u32b.copy_from_slice(&take(&mut cursor, 4, "rows")?);
            let frame_rows = u32::from_le_bytes(u32b);
            let mut u64b = [0u8; 8];
            u64b.copy_from_slice(&take(&mut cursor, 8, "offset")?);
            let offset = u64::from_le_bytes(u64b);
            u32b.copy_from_slice(&take(&mut cursor, 4, "len")?);
            let len = u32::from_le_bytes(u32b);
            frames.push(RawFrame {
                page_id,
                start_row,
                rows: frame_rows,
                offset,
                len,
            });
        }
        raw_chunks.push(RawChunk {
            row_offset,
            rows,
            encoding,
            meta,
            uncompressed_bytes: u64::from_le_bytes(uncompressed) as usize,
            frames,
        });
    }
    let payloads = cursor;
    let mut chunks = Vec::with_capacity(raw_chunks.len());
    for raw in raw_chunks {
        let mut frames = Vec::with_capacity(raw.frames.len());
        let mut expected = 0u64;
        for frame in raw.frames {
            if frame.offset != expected {
                return Err(snapshot_error(format!(
                    "payload gap at chunk offset {}: frame at {}",
                    raw.row_offset, frame.offset
                )));
            }
            let start = frame.offset as usize;
            let end = start.saturating_add(4).saturating_add(frame.len as usize);
            let frame_bytes = payloads.get(start..end).ok_or_else(|| {
                snapshot_error(format!(
                    "payload frame out of range at chunk offset {}",
                    raw.row_offset
                ))
            })?;
            let stored_len = u32::from_le_bytes(
                frame_bytes[..4]
                    .try_into()
                    .map_err(|_| snapshot_error("payload length prefix malformed".to_string()))?,
            );
            if stored_len != frame.len {
                return Err(snapshot_error(format!(
                    "payload length mismatch at chunk offset {}",
                    raw.row_offset
                )));
            }
            expected = end as u64;
            frames.push(MappedFrame {
                page_id: frame.page_id,
                start_row: frame.start_row,
                rows: frame.rows,
                offset: frame.offset,
                len: frame.len,
            });
        }
        if expected != payloads.len() as u64 {
            return Err(snapshot_error(format!(
                "payload trailing bytes at chunk offset {}",
                raw.row_offset
            )));
        }
        chunks.push(MappedChunk {
            row_offset: raw.row_offset,
            rows: raw.rows,
            encoding: raw.encoding,
            meta: raw.meta,
            uncompressed_bytes: raw.uncompressed_bytes,
            frames,
        });
    }
    // Frame offsets are relative to the payload region, but readers slice
    // the whole mapping: shift every frame past the record prefix.
    let payload_base = (bytes.len() - 4 - payloads.len()) as u64;
    for chunk in &mut chunks {
        for frame in &mut chunk.frames {
            frame.offset += payload_base;
        }
    }
    Ok(MappedSnapshot { map, chunks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spilled_snapshot_roundtrips_values_and_deletes_file() {
        let values = vec![Some(Value::Int(7)), None, Some(Value::string("hello"))];
        let snapshot = EvictedSnapshot::capture_spilled(
            0,
            values.clone(),
            EncodingType::None,
            ChunkEncodingMeta::default(),
        )
        .expect("spill must succeed");
        assert!(snapshot.is_spilled());
        assert_eq!(snapshot.resident_bytes(), 0);
        assert!(snapshot.compressed_bytes() > 0);

        let path = snapshot
            .spill_file_path()
            .expect("spilled snapshot has a file");
        assert!(path.exists());

        assert_eq!(snapshot.decode_row(0).unwrap(), Some(Value::Int(7)));
        assert_eq!(snapshot.decode_row(1).unwrap(), None);
        assert_eq!(
            snapshot.decode_row(2).unwrap(),
            Some(Value::string("hello"))
        );
        let all = snapshot.decode_all().unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], (0, Some(Value::Int(7))));

        drop(snapshot);
        assert!(!path.exists());
    }

    #[test]
    fn memory_snapshot_stays_on_heap() {
        let snapshot = EvictedSnapshot::capture(
            0,
            vec![Some(Value::Int(1))],
            EncodingType::None,
            ChunkEncodingMeta::default(),
        )
        .expect("capture must succeed");
        assert!(!snapshot.is_spilled());
        assert!(snapshot.spill_file_path().is_none());
        assert!(snapshot.resident_bytes() > 0);
    }

    fn unique_sidecar_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "vtx-sidecar-{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn sidecar_roundtrips_mapped_pages() {
        let values = vec![Some(Value::Int(11)), None, Some(Value::Int(13))];
        let snapshot = EvictedSnapshot::capture(
            0,
            values.clone(),
            EncodingType::None,
            ChunkEncodingMeta::default(),
        )
        .expect("capture must succeed");
        let pages = snapshot.compressed_pages().expect("pages materialize");
        assert_eq!(pages.len(), 1);
        let plan = SnapshotChunkPlan {
            row_offset: 0,
            rows: 3,
            encoding: EncodingType::None,
            meta: &ChunkEncodingMeta::default(),
            uncompressed_bytes: snapshot.uncompressed_bytes as u64,
            pages,
        };
        let bytes = encode_snapshot_sidecar(std::slice::from_ref(&plan)).expect("encode works");
        let path = unique_sidecar_path("roundtrip");
        std::fs::write(&path, &bytes).expect("sidecar write works");
        let mapped = open_snapshot_sidecar(&path).expect("sidecar opens");
        assert_eq!(mapped.chunks.len(), 1);
        let record = mapped.chunks.into_iter().next().expect("one chunk");
        assert_eq!((record.row_offset, record.rows), (0, 3));
        let restored = EvictedSnapshot::from_mapped(
            record.rows,
            record.encoding,
            record.meta,
            record.uncompressed_bytes,
            mapped.map,
            record.frames,
        );
        assert!(restored.is_mapped());
        assert_eq!(restored.resident_bytes(), 0);
        assert_eq!(restored.decode_row(0).unwrap(), Some(Value::Int(11)));
        assert_eq!(restored.decode_row(1).unwrap(), None);
        assert_eq!(restored.decode_row(2).unwrap(), Some(Value::Int(13)));
        assert_eq!(restored.decode_all().unwrap().len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sidecar_rejects_corruption() {
        let snapshot = EvictedSnapshot::capture(
            0,
            vec![Some(Value::Int(1))],
            EncodingType::None,
            ChunkEncodingMeta::default(),
        )
        .expect("capture must succeed");
        let pages = snapshot.compressed_pages().expect("pages materialize");
        let plan = SnapshotChunkPlan {
            row_offset: 0,
            rows: 1,
            encoding: EncodingType::None,
            meta: &ChunkEncodingMeta::default(),
            uncompressed_bytes: snapshot.uncompressed_bytes as u64,
            pages,
        };
        let mut bytes = encode_snapshot_sidecar(std::slice::from_ref(&plan)).expect("encode works");
        bytes[10] ^= 0xff;
        let path = unique_sidecar_path("corrupt");
        std::fs::write(&path, &bytes).expect("sidecar write works");
        assert!(open_snapshot_sidecar(&path).is_err());
        let _ = std::fs::write(&path, b"junk");
        assert!(open_snapshot_sidecar(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
