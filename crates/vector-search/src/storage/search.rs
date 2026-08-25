//! Search execution for a collection: routing to the published ANN tier or
//! the exact parallel scan, plus shared post-processing.
//!
//! All paths take their metadata and per-file views inside one read-lock
//! acquisition (so a concurrent background compaction cannot produce a
//! mixed-generation view), run on immutable snapshots without locks, and
//! share identical post-processing through [`finish_candidates`]: payload
//! filter, score threshold, top-K, offset/limit. Scores and semantics are
//! therefore path-independent.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::sync::Arc;

use rayon::prelude::*;

use super::directory::DirView;
use super::index_lifecycle::PublishedIndex;
use super::keys::Keys;
use super::payloads::Payloads;
use super::vectors::Vectors;
use super::{CollectionStore, TombstoneBits};
use crate::error::{Result, VectorSearchError};
use crate::index::{HnswIndex, IvfIndex};
use crate::types::{PointId, SearchQuery, SearchResult};

/// Snapshot of the immutable views used by a single search pass.
struct SearchSnapshot<'a> {
    dim: usize,
    segment_slots: u32,
    tombstones: &'a TombstoneBits,
    vsnap: &'a [Arc<memmap2::Mmap>],
    keysnap: &'a DirView,
    paysnap: &'a DirView,
}

impl CollectionStore {
    /// Search: route to the published ANN tier (HNSW by default) or the
    /// exact full scan. All paths share identical post-processing (payload
    /// filter, score threshold, top-K, offset/limit) through
    /// [`Self::finish_candidates`], so scores and semantics are
    /// path-independent.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        // Take the metadata and all per-file views inside one read-lock
        // acquisition so a concurrent background compaction (which swaps
        // every file under the write lock) cannot produce a mixed-generation
        // view. Only the atomic loads happen under the lock; the actual scan
        // runs on immutable snapshots.
        let (dim, metric, segment_slots, next_slot, tombstones, vsnap, keysnap, paysnap, published) = {
            let inner = self.inner.read();
            if query.vector.len() != inner.meta.vector_size {
                return Err(VectorSearchError::InvalidVectorDimension {
                    expected: inner.meta.vector_size,
                    actual: query.vector.len(),
                });
            }
            (
                inner.meta.vector_size,
                inner.meta.distance,
                inner.meta.segment_slots,
                inner.meta.next_slot,
                self.tombstones.load(),
                self.vectors.snapshot(),
                self.keys.snapshot(),
                self.payloads.snapshot(),
                self.index.load(),
            )
        };
        match published.as_ref() {
            Some(PublishedIndex::Ivf(index)) => {
                let results = self.search_ivf(
                    index.as_ref(),
                    query,
                    dim,
                    segment_slots,
                    &tombstones,
                    &vsnap,
                    &keysnap,
                    &paysnap,
                );
                drop(published);
                return results;
            }
            Some(PublishedIndex::Hnsw(index)) => {
                let results = self.search_hnsw(
                    index.as_ref(),
                    query,
                    dim,
                    segment_slots,
                    &tombstones,
                    &vsnap,
                    &keysnap,
                    &paysnap,
                );
                drop(published);
                return results;
            }
            None => {}
        }
        drop(published);

        // 1. Parallel exact scan, skipping tombstones.
        let candidates: Vec<(f32, u32)> = (0..next_slot as u32)
            .into_par_iter()
            .filter(|s| !tombstones.bit(*s as usize))
            .map(|s| {
                let v =
                    Vectors::read_slot(&vsnap, s as u64, segment_slots, dim).ok_or_else(|| {
                        VectorSearchError::CorruptData(format!("slot {s} out of vectors.bin range"))
                    })?;
                let dist = crate::distance::distance(metric, &query.vector, v);
                Ok((crate::distance::to_score(metric, dist), s))
            })
            .collect::<Result<Vec<_>>>()?;

        self.finish_candidates(
            candidates,
            query,
            &SearchSnapshot {
                dim,
                segment_slots,
                tombstones: &tombstones,
                vsnap: &vsnap,
                keysnap: &keysnap,
                paysnap: &paysnap,
            },
        )
    }

    /// IVF path: probe the `nprobe` closest lists (+ pending slots), then
    /// shared post-processing. With a payload filter that leaves fewer than
    /// `limit` results while unprobed lists remain, nprobe is doubled once as
    /// a bounded accuracy fallback; beyond that the approximate semantics of
    /// IVFFlat apply (`nprobe = lists` degenerates to exact).
    #[allow(clippy::too_many_arguments)]
    fn search_ivf(
        &self,
        index: &IvfIndex,
        query: &SearchQuery,
        dim: usize,
        segment_slots: u32,
        tombstones: &TombstoneBits,
        vsnap: &[Arc<memmap2::Mmap>],
        keysnap: &DirView,
        paysnap: &DirView,
    ) -> Result<Vec<SearchResult>> {
        let lists = index.list_count();
        let mut nprobe = index.clamp_nprobe(query.nprobe);
        let pending = self.pending.read().clone();

        let candidates = index.probe_candidates(
            &query.vector,
            nprobe,
            &pending,
            tombstones,
            vsnap,
            segment_slots,
        )?;
        let results = self.finish_candidates(
            candidates,
            query,
            &SearchSnapshot {
                dim,
                segment_slots,
                tombstones,
                vsnap,
                keysnap,
                paysnap,
            },
        )?;

        let short = query.filter.is_some() && results.len() < query.limit;
        if !short || nprobe >= lists {
            return Ok(results);
        }
        // Single controlled retry with a doubled probe width.
        nprobe = (nprobe * 2).min(lists);
        let candidates = index.probe_candidates(
            &query.vector,
            nprobe,
            &pending,
            tombstones,
            vsnap,
            segment_slots,
        )?;
        self.finish_candidates(
            candidates,
            query,
            &SearchSnapshot {
                dim,
                segment_slots,
                tombstones,
                vsnap,
                keysnap,
                paysnap,
            },
        )
    }

    /// HNSW path: layered-graph search with `ef` from the query's
    /// `SearchMode::KNN.ef_search`, falling back to the index default. With a
    /// payload filter that leaves fewer than `limit` results, `ef` is doubled
    /// once as a bounded accuracy fallback (mirroring the IVF nprobe retry).
    ///
    /// Pending slots (not yet incorporated into the graph) are scored by
    /// brute force and merged with graph candidates, ensuring visibility
    /// matches the IVF path's `extra_slots` semantics.
    #[allow(clippy::too_many_arguments)]
    fn search_hnsw(
        &self,
        index: &HnswIndex,
        query: &SearchQuery,
        dim: usize,
        segment_slots: u32,
        tombstones: &TombstoneBits,
        vsnap: &[Arc<memmap2::Mmap>],
        keysnap: &DirView,
        paysnap: &DirView,
    ) -> Result<Vec<SearchResult>> {
        let snap = SearchSnapshot {
            dim,
            segment_slots,
            tombstones,
            vsnap,
            keysnap,
            paysnap,
        };
        let mut ef = query.hnsw_ef().unwrap_or_else(|| index.default_ef());
        let mut candidates = index.probe_candidates(
            &query.vector,
            ef,
            query.effective_limit(),
            tombstones,
            vsnap,
            segment_slots,
        )?;

        // Merge pending slots (exact brute-force scoring) so points
        // inserted after the index was published remain visible.
        let pending = self.pending.read().clone();
        if !pending.is_empty() {
            let metric = self.inner.read().meta.distance;
            let pending_scored: Vec<(f32, u32)> = pending
                .par_iter()
                .copied()
                .filter(|&s| !tombstones.bit(s as usize))
                .filter_map(|s| {
                    let v = Vectors::read_slot(vsnap, s as u64, segment_slots, dim)?;
                    let dist = crate::distance::distance(metric, &query.vector, v);
                    Some((crate::distance::to_score(metric, dist), s))
                })
                .collect();
            // Deduplicate: if a slot appears in both graph results and
            // pending, keep the better score (pending uses the live vector
            // so scores are equivalent; just avoid duplicates).
            let mut seen: HashSet<u32> = candidates.iter().map(|&(_, s)| s).collect();
            for scored @ (_, slot) in pending_scored {
                if seen.insert(slot) {
                    candidates.push(scored);
                }
            }
        }

        let results = self.finish_candidates(candidates, query, &snap)?;

        let short = query.filter.is_some() && results.len() < query.limit;
        if !short {
            return Ok(results);
        }
        // Single controlled retry with a doubled candidate list; the graph
        // has at most `node_count` live results so growing past it is moot.
        let cap = index.node_count().max(1);
        if ef >= cap {
            return Ok(results);
        }
        ef = (ef * 2).min(cap);
        let mut candidates = index.probe_candidates(
            &query.vector,
            ef,
            query.effective_limit(),
            tombstones,
            vsnap,
            segment_slots,
        )?;
        // Re-merge pending on retry.
        let pending = self.pending.read().clone();
        if !pending.is_empty() {
            let metric = self.inner.read().meta.distance;
            let pending_scored: Vec<(f32, u32)> = pending
                .par_iter()
                .copied()
                .filter(|&s| !tombstones.bit(s as usize))
                .filter_map(|s| {
                    let v = Vectors::read_slot(vsnap, s as u64, segment_slots, dim)?;
                    let dist = crate::distance::distance(metric, &query.vector, v);
                    Some((crate::distance::to_score(metric, dist), s))
                })
                .collect();
            let mut seen: HashSet<u32> = candidates.iter().map(|&(_, s)| s).collect();
            for scored @ (_, slot) in pending_scored {
                if seen.insert(slot) {
                    candidates.push(scored);
                }
            }
        }
        self.finish_candidates(candidates, query, &snap)
    }

    /// Shared post-processing for both search paths: payload post-filter,
    /// score threshold, top-K heap selection and result assembly.
    fn finish_candidates(
        &self,
        mut candidates: Vec<(f32, u32)>,
        query: &SearchQuery,
        snap: &SearchSnapshot,
    ) -> Result<Vec<SearchResult>> {
        let SearchSnapshot {
            dim,
            segment_slots,
            tombstones,
            vsnap,
            keysnap,
            paysnap,
        } = snap;
        let _ = tombstones;
        // 2. Post-filter on payload, against the snapshots taken by the
        // caller (one compaction generation).
        if let Some(filter) = &query.filter {
            let mut kept = Vec::with_capacity(candidates.len());
            for (score, slot) in candidates {
                let key = Keys::read_key(keysnap, slot as usize)?.ok_or_else(|| {
                    VectorSearchError::CorruptData(format!("slot {slot} has no key"))
                })?;
                let id = PointId::from(key);
                let payload = Payloads::read_payload(paysnap, slot as usize)?;
                if crate::filter::matches(filter, &id, payload.as_ref())? {
                    kept.push((score, slot));
                }
            }
            candidates = kept;
        }

        // 3. Score threshold (lower bound on the output score).
        if let Some(threshold) = query.score_threshold {
            candidates.retain(|(score, _)| *score >= threshold);
        }

        // 4. Top-K by score (K = offset + limit), then sort descending.
        let k = query.offset.unwrap_or(0).saturating_add(query.limit);
        let top: Vec<(f32, u32)> = if candidates.len() <= k {
            candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
            candidates
        } else {
            let mut heap: BinaryHeap<std::cmp::Reverse<ScoredSlot>> = BinaryHeap::new();
            for (score, slot) in candidates {
                let s = ScoredSlot { score, slot };
                if heap.len() < k {
                    heap.push(std::cmp::Reverse(s));
                } else if heap.peek().is_some_and(|top| s > top.0) {
                    heap.pop();
                    heap.push(std::cmp::Reverse(s));
                }
            }
            let mut v: Vec<(f32, u32)> = heap
                .into_iter()
                .map(|std::cmp::Reverse(s)| (s.score, s.slot))
                .collect();
            v.sort_by(|a, b| b.0.total_cmp(&a.0));
            v
        };

        // 5. Assemble results against the same snapshots the scan used.
        let with_payload = query
            .with_payload
            .unwrap_or(crate::types::DEFAULT_WITH_PAYLOAD);
        let with_vector = query.with_vector.unwrap_or(false);
        let mut results = Vec::new();
        for (score, slot) in top
            .into_iter()
            .skip(query.offset.unwrap_or(0))
            .take(query.limit)
        {
            let key = Keys::read_key(keysnap, slot as usize)?
                .ok_or_else(|| VectorSearchError::CorruptData(format!("slot {slot} has no key")))?;
            let mut result = SearchResult::new(PointId::from(key), score);
            if with_payload {
                if let Some(payload) = Payloads::read_payload(paysnap, slot as usize)? {
                    result = result.with_payload(payload);
                }
            }
            if with_vector {
                let v = Vectors::read_slot(vsnap, slot as u64, *segment_slots, *dim).ok_or_else(
                    || {
                        VectorSearchError::CorruptData(format!(
                            "slot {slot} out of vectors.bin range"
                        ))
                    },
                )?;
                result = result.with_vector(v.to_vec());
            }
            results.push(result);
        }
        Ok(results)
    }
}

/// A (score, slot) pair ordered by score for the top-K heap.
#[derive(Clone, Copy, Debug)]
struct ScoredSlot {
    score: f32,
    slot: u32,
}

impl PartialEq for ScoredSlot {
    fn eq(&self, other: &Self) -> bool {
        self.score.to_bits() == other.score.to_bits() && self.slot == other.slot
    }
}

impl Eq for ScoredSlot {}

impl PartialOrd for ScoredSlot {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredSlot {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then(self.slot.cmp(&other.slot))
    }
}
