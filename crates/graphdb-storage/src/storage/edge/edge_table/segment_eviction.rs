//! Segment eviction engine for memory-aware tiering.
//!
//! Provides the logic for selecting and evicting cold segments when memory
//! pressure exceeds the soft limit. Evicted segments have their CSR data
//! serialized to spill files and physical memory freed; subsequent access
//! triggers transparent reload.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::residency::AccessClock;
use super::segment::CsrSegment;
use crate::core::{StorageError, StorageResult};

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
pub struct SegmentEvictionEngine {
    /// Directory for spill files.
    spill_dir: PathBuf,
    /// Monotonic clock for LRU tracking.
    access_clock: Arc<AccessClock>,
}

impl SegmentEvictionEngine {
    pub fn new(spill_dir: PathBuf) -> Self {
        Self {
            spill_dir,
            access_clock: Arc::new(AccessClock::new()),
        }
    }

    pub fn access_clock(&self) -> &AccessClock {
        &self.access_clock
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
                if best
                    .as_ref()
                    .is_none_or(|b| candidate.last_access < b.last_access)
                {
                    best = Some(candidate);
                }
            }
        }

        for (idx, segment) in table.in_segments.iter().enumerate() {
            if let Some(candidate) = self.evaluate_candidate(segment, Direction::In, idx) {
                if best
                    .as_ref()
                    .is_none_or(|b| candidate.last_access < b.last_access)
                {
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
        if !segment.is_resident() {
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

    /// Evict a single segment: two-pass approach.
    /// Pass 1: Mark the segment (still readable). Pass 2: Complete eviction.
    /// This gives optimistic readers a second chance to finish.
    fn evict_segment(
        &self,
        segment: &CsrSegment,
        direction: Direction,
        index: usize,
    ) -> StorageResult<usize> {
        let spill_path = self
            .spill_dir
            .join(format!("{:?}_{}.spill", direction, index));

        // If already marked, complete the eviction
        if segment.lock_state.read_state() == super::page_state::SegmentState::Marked {
            let bytes = segment.finish_eviction(&spill_path)?;
            return Ok(bytes as usize);
        }

        // First pass: mark for eviction (second chance for readers)
        if segment.begin_eviction() {
            return Ok(0); // Will be completed on next pass
        }

        Err(StorageError::invalid_operation(
            "segment is locked by writer".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_clock_shared() {
        let engine = SegmentEvictionEngine::new(PathBuf::from("/tmp"));
        let clock = engine.access_clock();
        let t1 = clock.tick();
        let t2 = engine.access_clock().tick();
        assert!(t1 < t2);
    }
}
