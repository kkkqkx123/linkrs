//! Compaction helpers: rewriting the directory files (`keys.bin` /
//! `payloads.bin`) and `vectors.bin` with a compacted slot numbering.
//!
//! Compaction runs under the store's write lock. It builds temp files, fsyncs
//! them, then atomically renames each over the live file (the `replace_from`
//! path), which is invisible to readers holding old mmap snapshots.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use memmap2::Mmap;

use crate::error::{Result, VectorSearchError};

const HEADER_LEN: usize = 24;
const FILE_VERSION: u32 = 1;

/// One entry of a rebuilt directory file: blob for `slot` with `flags`.
pub(crate) struct DirEntry {
    pub slot: u32,
    pub blob: Vec<u8>,
    pub flags: u8,
}

/// Compute the old-slot -> new-slot mapping for all live slots.
///
/// `is_live(slot)` reports whether the slot survives compaction. Returns the
/// new capacity (segment-aligned, at least one segment) and the mapping array:
/// `map[old_slot] = new_slot` for live slots.
pub(crate) fn plan_slots(
    mut is_live: impl FnMut(usize) -> bool,
    next_slot: u64,
    segment_slots: u32,
) -> (u64, Vec<u32>) {
    let mut map = vec![u32::MAX; next_slot as usize];
    let mut live = 0u32;
    for (slot, entry) in map.iter_mut().enumerate() {
        if is_live(slot) {
            *entry = live;
            live += 1;
        }
    }
    let new_capacity = (live as u64).max(1).div_ceil(segment_slots as u64) * segment_slots as u64;
    (new_capacity, map)
}

/// Write a complete directory file (header + record array + blob area) to
/// `path`, then fsync.
pub(crate) fn write_dir_file(
    path: &Path,
    magic: [u8; 4],
    rec_size: usize,
    rec_capacity: u64,
    entries: &[DirEntry],
) -> Result<()> {
    let blob_len: u64 = entries.iter().map(|e| e.blob.len() as u64).sum();

    let mut file = File::create(path)?;
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(&magic);
    header.extend_from_slice(&FILE_VERSION.to_le_bytes());
    header.extend_from_slice(&rec_capacity.to_le_bytes());
    header.extend_from_slice(&blob_len.to_le_bytes());
    file.write_all(&header)?;

    let rec_array_len = rec_capacity as usize * rec_size;
    file.write_all(&vec![0u8; rec_array_len])?;

    let mut blob_offset = 0u32;
    for e in entries {
        let rec_start = HEADER_LEN + e.slot as usize * rec_size;
        let mut rec = Vec::with_capacity(rec_size);
        rec.extend_from_slice(&blob_offset.to_le_bytes());
        rec.extend_from_slice(&(e.blob.len() as u32).to_le_bytes());
        if rec_size >= 12 {
            rec.push(e.flags);
            rec.extend_from_slice(&[0u8; 3]);
        }
        rec.truncate(rec_size);
        write_at(&mut file, &rec, rec_start as u64)?;
        if !e.blob.is_empty() {
            let blob_start = HEADER_LEN + rec_array_len + blob_offset as usize;
            write_at(&mut file, &e.blob, blob_start as u64)?;
        }
        blob_offset += e.blob.len() as u32;
    }
    file.sync_all()?;
    Ok(())
}

/// Build a fresh dense `vectors.bin` at `path` (temp file) containing the
/// vectors of the live slots in new-slot order.
pub(crate) fn write_vectors_file(
    path: &Path,
    dim: usize,
    segment_slots: u32,
    new_capacity: u64,
    old_vectors: &[Arc<Mmap>],
    map: &[u32],
) -> Result<()> {
    let total = new_capacity as usize * dim * 4;
    let mut file = File::create(path)?;
    file.set_len(total as u64)?;
    file.sync_all()?;

    for (old_slot, new_slot) in map.iter().enumerate() {
        if *new_slot == u32::MAX {
            continue;
        }
        let seg_idx = old_slot / segment_slots as usize;
        let in_seg = old_slot % segment_slots as usize;
        let seg = old_vectors.get(seg_idx).ok_or_else(|| {
            VectorSearchError::CorruptData(format!("slot {old_slot} out of vectors.bin range"))
        })?;
        let offset = in_seg * dim * 4;
        let end = offset + dim * 4;
        if end > seg.len() {
            return Err(VectorSearchError::CorruptData(format!(
                "slot {old_slot} out of vectors.bin range"
            )));
        }
        let bytes = &seg[offset..end];
        let dst = *new_slot as usize * dim * 4;
        if dst + bytes.len() > total {
            return Err(VectorSearchError::Internal(
                "compaction vector write out of bounds".to_string(),
            ));
        }
        write_at(&mut file, bytes, dst as u64)?;
    }
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &mut File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

#[cfg(not(unix))]
fn write_at(file: &mut File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(buf)
}
