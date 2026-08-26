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
use std::time::Instant;

use bitvec::prelude::*;
use rayon::prelude::*;

use super::directory::DirView;
use super::index_lifecycle::PublishedIndex;
use super::keys::Keys;
use super::payloads::Payloads;
use super::vectors::Vectors;
use super::{CollectionStore, TombstoneBits};
use crate::error::{Result, VectorSearchError};
use crate::index::hnsw::HnswSearchContext;
use crate::index::IvfSearchContext;
use crate::index::{HnswIndex, IvfIndex};
use crate::metrics::{SearchPath, SearchRetry};
use crate::types::{PointId, SearchQuery, SearchResult};

/// Snapshot of the immutable views used by a single search pass.
struct SearchSnapshot<'a> {
    dim: usize,
    segment_slots: u32,
    tombstones: &'a TombstoneBits,
    vsnap: &'a [Arc<memmap2::Mmap>],
    keysnap: &'a DirView,
    paysnap: &'a DirView,
    filter_mask: Option<&'a BitVec>,
}

impl CollectionStore {
    /// Search: route to the published ANN tier (HNSW by default) or the
    /// exact full scan. All paths share identical post-processing (payload
    /// filter, score threshold, top-K, offset/limit) through
    /// [`Self::finish_candidates`], so scores and semantics are
    /// path-independent.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let started = Instant::now();
        // Take the metadata and all per-file views inside one read-lock
        // acquisition so a concurrent background compaction (which swaps
        // every file under the write lock) cannot produce a mixed-generation
        // view. Only the atomic loads happen under the lock; the actual scan
        // runs on immutable snapshots. `pending` must be captured here too:
        // a slot enters pending only after its key/payload/vector are
        // visible in the current file generations (both happen under the
        // store write lock), so a pending slot is always covered by these
        // snapshots. Loading it later could surface a slot whose key is
        // missing from the older key generation.
        let (
            dim,
            metric,
            segment_slots,
            next_slot,
            tombstones,
            vsnap,
            keysnap,
            paysnap,
            published,
            filter_mask,
            pending,
        ) = {
            let inner = self.inner.read();
            if query.vector.len() != inner.meta.vector_size {
                return Err(VectorSearchError::InvalidVectorDimension {
                    expected: inner.meta.vector_size,
                    actual: query.vector.len(),
                });
            }
            let mask = query
                .filter
                .as_ref()
                .and_then(|f| inner.filter_bitmap.build_mask(f));
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
                mask,
                self.pending.load_full(),
            )
        };
        let filtered = query.filter.is_some();
        match published.as_ref() {
            Some(PublishedIndex::Ivf(index)) => {
                let snap = SearchSnapshot {
                    dim,
                    segment_slots,
                    tombstones: &tombstones,
                    vsnap: &vsnap,
                    keysnap: &keysnap,
                    paysnap: &paysnap,
                    filter_mask: filter_mask.as_ref(),
                };
                let results = self.search_ivf(index.as_ref(), query, &snap, &pending);
                self.metrics
                    .record_search(SearchPath::Ivf, filtered, started.elapsed());
                drop(published);
                return results;
            }
            Some(PublishedIndex::Hnsw(index)) => {
                let snap = SearchSnapshot {
                    dim,
                    segment_slots,
                    tombstones: &tombstones,
                    vsnap: &vsnap,
                    keysnap: &keysnap,
                    paysnap: &paysnap,
                    filter_mask: filter_mask.as_ref(),
                };
                let results = self.search_hnsw(index.as_ref(), query, &snap, &pending);
                self.metrics
                    .record_search(SearchPath::Hnsw, filtered, started.elapsed());
                drop(published);
                return results;
            }
            None => {}
        }
        drop(published);

        // 1. Parallel exact scan with streaming top-K. The parallel heap
        //    avoids materializing the full (score, slot) set when the
        //    collection is large, cutting peak memory to O(k).
        let k = query.offset.unwrap_or(0).saturating_add(query.limit);
        let heap: BinaryHeap<std::cmp::Reverse<ScoredSlot>> = (0..next_slot as u32)
            .into_par_iter()
            .filter(|s| !tombstones.bit(*s as usize))
            .fold(
                || BinaryHeap::with_capacity(k),
                |mut acc, s| {
                    self.exact_scan_step(
                        &mut acc,
                        s,
                        k,
                        metric,
                        query,
                        &SearchSnapshot {
                            dim,
                            segment_slots,
                            tombstones: &tombstones,
                            vsnap: &vsnap,
                            keysnap: &keysnap,
                            paysnap: &paysnap,
                            filter_mask: None,
                        },
                    );
                    acc
                },
            )
            .reduce(
                || BinaryHeap::with_capacity(k),
                |mut acc, other| {
                    for item in other.into_iter() {
                        if acc.len() < k {
                            acc.push(item);
                        } else if let Some(std::cmp::Reverse(min)) = acc.peek() {
                            if item.0 > *min {
                                acc.pop();
                                acc.push(item);
                            }
                        }
                    }
                    acc
                },
            );

        let mut candidates: Vec<(f32, u32)> = heap
            .into_iter()
            .map(|std::cmp::Reverse(s)| (s.score, s.slot))
            .collect();
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

        self.metrics
            .record_search(SearchPath::Exact, filtered, started.elapsed());
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
                filter_mask: None,
            },
        )
    }

    /// Single live-slot evaluation for the streaming exact scan.
    ///
    /// The payload filter is evaluated inline so the heap is only ever
    /// populated with candidates that pass the filter; stale data (missing
    /// key/payload) is silently skipped, mirroring the IVF index path's
    /// `filter_map` behaviour.
    fn exact_scan_step(
        &self,
        heap: &mut BinaryHeap<std::cmp::Reverse<ScoredSlot>>,
        slot: u32,
        k: usize,
        metric: crate::types::DistanceMetric,
        query: &SearchQuery,
        snap: &SearchSnapshot<'_>,
    ) {
        let SearchSnapshot {
            keysnap, paysnap, ..
        } = snap;
        if let Some(filter) = &query.filter {
            let Some(id) = Keys::read_key(keysnap, slot as usize)
                .ok()
                .flatten()
                .map(PointId::from)
            else {
                return;
            };
            let payload = Payloads::read_payload(paysnap, slot as usize)
                .ok()
                .flatten();
            let ok = crate::filter::matches(filter, &id, payload.as_ref()).unwrap_or(false);
            if !ok {
                return;
            }
        }
        let Some(v) = Vectors::read_slot(snap.vsnap, slot as u64, snap.segment_slots, snap.dim)
        else {
            return;
        };
        let dist = crate::distance::distance(metric, &query.vector, v);
        let score = crate::distance::to_score(metric, dist);
        let item = ScoredSlot { score, slot };
        if heap.len() < k {
            heap.push(std::cmp::Reverse(item));
        } else if let Some(std::cmp::Reverse(min)) = heap.peek() {
            if item > *min {
                heap.pop();
                heap.push(std::cmp::Reverse(item));
            }
        }
    }

    /// IVF path: probe the `nprobe` closest lists (+ pending slots), then
    /// shared post-processing. With a payload filter that leaves fewer than
    /// `limit` results, the probe width keeps doubling (each widening
    /// recorded as a [`SearchRetry::NprobeDoubling`]) until the result count
    /// is met, every list has been probed, or `IvfConfig::max_probes` caps
    /// the widening — pgvector's multi-round iterative scan semantics.
    fn search_ivf(
        &self,
        index: &IvfIndex,
        query: &SearchQuery,
        snap: &SearchSnapshot<'_>,
        pending: &[u32],
    ) -> Result<Vec<SearchResult>> {
        let mut nprobe = index.clamp_nprobe(query.nprobe);
        let cap = index.max_probe_cap();

        let ivf_ctx = IvfSearchContext {
            tombstones: snap.tombstones,
            vectors: snap.vsnap,
            segment_slots: snap.segment_slots,
            filter_mask: snap.filter_mask,
        };
        loop {
            let candidates = index.probe_candidates(&query.vector, nprobe, pending, &ivf_ctx)?;
            let results = self.finish_candidates(candidates, query, snap)?;

            let short = query.filter.is_some() && results.len() < query.limit;
            if !short || nprobe >= cap {
                return Ok(results);
            }
            self.metrics
                .record_search_retry(SearchRetry::NprobeDoubling);
            nprobe = (nprobe * 2).min(cap);
        }
    }

    /// HNSW path: layered-graph search with `ef` from the query's
    /// `SearchMode::KNN.ef_search`, falling back to the index default. With a
    /// payload filter that leaves fewer than `limit` results, iterative scan
    /// expansion is attempted before falling back to doubling `ef`.
    ///
    /// Pending slots (not yet incorporated into the graph) are scored by
    /// brute force and merged with graph candidates, ensuring visibility
    /// matches the IVF path's `extra_slots` semantics.
    fn search_hnsw(
        &self,
        index: &HnswIndex,
        query: &SearchQuery,
        snap: &SearchSnapshot<'_>,
        pending: &[u32],
    ) -> Result<Vec<SearchResult>> {
        let mut ef = query.hnsw_ef().unwrap_or_else(|| index.default_ef());
        let hnsw_ctx = HnswSearchContext {
            tombstones: Some(snap.tombstones),
            vectors: snap.vsnap,
            segment_slots: snap.segment_slots,
            filter_mask: snap.filter_mask,
        };
        let candidates =
            index.probe_candidates(&query.vector, ef, query.effective_limit(), &hnsw_ctx)?;
        let candidates = self.merge_pending(candidates, pending, query, snap);

        let results = self.finish_candidates(candidates, query, snap)?;

        let short = query.filter.is_some() && results.len() < query.limit;
        if !short {
            return Ok(results);
        }
        // Iterative expansion: try resuming from discarded candidates before
        // doubling ef. This mirrors pgvector's iterative scan which recovers
        // evicted candidates rather than widening the search window.
        let cap = index.node_count().max(1);
        if ef >= cap {
            return Ok(results);
        }
        self.metrics
            .record_search_retry(SearchRetry::IterativeExpansion);
        let iterative_results = index.probe_candidates_iterative(
            &query.vector,
            ef,
            query.effective_limit(),
            index.iterative_rounds(),
            index.scan_tuple_budget(),
            &hnsw_ctx,
        );
        if let Ok(iterative_candidates) = iterative_results {
            let candidates = self.merge_pending(iterative_candidates, pending, query, snap);
            return self.finish_candidates(candidates, query, snap);
        }

        // Fallback: double ef for a single controlled retry.
        self.metrics.record_search_retry(SearchRetry::EfDoubling);
        ef = (ef * 2).min(cap);
        let candidates =
            index.probe_candidates(&query.vector, ef, query.effective_limit(), &hnsw_ctx)?;
        let candidates = self.merge_pending(candidates, pending, query, snap);
        self.finish_candidates(candidates, query, snap)
    }

    /// Score pending slots by brute force against the live vectors and merge
    /// them into ANN candidates (deduplicating in favor of the first
    /// occurrence), so points inserted after a build stay visible.
    fn merge_pending(
        &self,
        mut candidates: Vec<(f32, u32)>,
        pending: &[u32],
        query: &SearchQuery,
        snap: &SearchSnapshot<'_>,
    ) -> Vec<(f32, u32)> {
        if pending.is_empty() {
            return candidates;
        }
        let metric = self.inner.read().meta.distance;
        let scored: Vec<(f32, u32)> = pending
            .par_iter()
            .copied()
            .filter(|&s| !snap.tombstones.bit(s as usize))
            .filter(|&s| {
                snap.filter_mask
                    .is_none_or(|m| (s as usize) < m.len() && m[s as usize])
            })
            .filter_map(|s| {
                let v = Vectors::read_slot(snap.vsnap, s as u64, snap.segment_slots, snap.dim)?;
                let dist = crate::distance::distance(metric, &query.vector, v);
                Some((crate::distance::to_score(metric, dist), s))
            })
            .collect();
        let mut seen: HashSet<u32> = candidates.iter().map(|&(_, s)| s).collect();
        for scored @ (_, slot) in scored {
            if seen.insert(slot) {
                candidates.push(scored);
            }
        }
        candidates
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
            filter_mask,
        } = snap;
        let _ = tombstones;
        let _ = filter_mask;
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
            candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
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
            v.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
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
