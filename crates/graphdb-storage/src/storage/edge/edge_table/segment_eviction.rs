//! Segment eviction engine for memory-aware tiering.
//!
//! Provides the logic for selecting and evicting cold segments when memory
//! pressure exceeds the soft limit. Evicted segments have their CSR data
//! serialized to spill files and physical memory freed; subsequent access
//! triggers transparent reload.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use super::residency::GLOBAL_ACCESS_CLOCK;
use super::segment::CsrSegment;
use crate::core::StorageResult;

/// Direction of edge traversal for segment selection.
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Out,
    In,
}

/// Candidate segment for eviction, ordered by LRU timestamp.
#[derive(Debug)]
struct EvictionCandidate<'a> {
    segment: &'a CsrSegment,
    direction: Direction,
    index: usize,
    last_access: u64,
    memory_bytes: usize,
}

/// Segment eviction engine.
///
/// Selects cold segments for eviction based on LRU ordering and performs
/// the eviction (serialize to spill file + free memory).
///
/// All instances share [`GLOBAL_ACCESS_CLOCK`] so that recorded access
/// timestamps and eviction ordering use the same monotonic counter.
pub struct SegmentEvictionEngine {
    /// Directory for spill files.
    spill_dir: PathBuf,
}

impl SegmentEvictionEngine {
    pub fn new(spill_dir: PathBuf) -> Self {
        Self { spill_dir }
    }

    /// Current access clock value (monotonic counter, not wall time).
    /// Delegates to the global shared clock.
    fn clock_now(&self) -> u64 {
        GLOBAL_ACCESS_CLOCK.now()
    }

    /// Evict coldest segments from a single edge table until `target_bytes`
    /// have been freed or no more eviction candidates exist.
    ///
    /// Returns the total bytes freed.
    pub fn evict_cold_segments(
        &self,
        table: &super::EdgeStore,
        target_bytes: usize,
    ) -> StorageResult<usize> {
        let _now = self.clock_now();
        let super::EdgeStore::TimeTravel(tt) = table;
        let mut freed = 0;

        while freed < target_bytes {
            let candidate = self.find_coldest_candidate(tt);
            match candidate {
                Some(ev) => {
                    let bytes = self.evict_segment(ev.segment, ev.direction, ev.index)?;
                    freed += bytes;
                }
                None => break,
            }
        }

        Ok(freed)
    }

    /// Find the coldest (least recently used) segment across both directions.
    fn find_coldest_candidate<'a>(
        &self,
        table: &'a super::core::TimeTravelEdgeStore,
    ) -> Option<EvictionCandidate<'a>> {
        let mut best: Option<EvictionCandidate<'a>> = None;

        for (idx, segment) in table.out_segments.iter().enumerate() {
            if let Some(candidate) = self.evaluate_candidate(segment, Direction::Out, idx) {
                if best.as_ref().is_none_or(|b| {
                    candidate.last_access < b.last_access
                        || (candidate.last_access == b.last_access
                            && candidate.memory_bytes > b.memory_bytes)
                }) {
                    best = Some(candidate);
                }
            }
        }

        for (idx, segment) in table.in_segments.iter().enumerate() {
            if let Some(candidate) = self.evaluate_candidate(segment, Direction::In, idx) {
                if best.as_ref().is_none_or(|b| {
                    candidate.last_access < b.last_access
                        || (candidate.last_access == b.last_access
                            && candidate.memory_bytes > b.memory_bytes)
                }) {
                    best = Some(candidate);
                }
            }
        }

        best
    }

    /// Evaluate a segment as a potential eviction candidate.
    /// Returns None if the segment is not eligible for eviction.
    fn evaluate_candidate<'a>(
        &self,
        segment: &'a CsrSegment,
        direction: Direction,
        index: usize,
    ) -> Option<EvictionCandidate<'a>> {
        // Use is_evicted() for clarity — it mirrors is_resident() but makes
        // the intent explicit when checking non-residency.
        if segment.is_evicted() {
            return None;
        }
        // Skip segments that are locked by writers or already in eviction pipeline
        if segment.lock_state.is_write_locked() {
            return None;
        }
        let bytes = segment.estimated_bytes();
        if bytes == 0 {
            return None;
        }
        let last_access = segment.last_access_ts.load(Ordering::Relaxed);
        Some(EvictionCandidate {
            segment,
            direction,
            index,
            last_access,
            memory_bytes: bytes,
        })
    }

    /// Evict a single segment delegating to the segment's own two-pass logic.
    ///
    /// After eviction, reads the `spill_size()` from the residency state for
    /// accurate freed-bytes accounting rather than relying on the return value
    /// of `evict_to_spill` (they are identical, but this path makes the intent
    /// and the data dependency on `SegmentResidency::spill_size` explicit).
    fn evict_segment(
        &self,
        segment: &CsrSegment,
        direction: Direction,
        index: usize,
    ) -> StorageResult<usize> {
        let spill_path = self
            .spill_dir
            .join(format!("{:?}_{}.spill", direction, index));
        let _ = segment.evict_to_spill(&spill_path)?;
        Ok(segment.spill_size() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eviction_engine_creation() {
        let engine = SegmentEvictionEngine::new(PathBuf::from("/tmp"));
        let _engine = engine;
    }

    #[test]
    fn test_global_clock_monotonic() {
        let t1 = super::residency::GLOBAL_ACCESS_CLOCK.tick();
        let t2 = super::residency::GLOBAL_ACCESS_CLOCK.tick();
        assert!(t1 < t2);
    }
}
