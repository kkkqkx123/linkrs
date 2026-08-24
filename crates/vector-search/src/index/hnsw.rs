//! HNSW graph index.
//!
//! Ported from the bundled pgvector 0.8.x reference implementation
//! (`ref/pgvector/src/hnsw*.c`) and adapted to this engine's slot/mmap
//! storage model:
//!
//! - Nodes are slots; vector values stay in `vectors.bin` and are read
//!   through mmap snapshots exactly like the IVF and exact-scan paths.
//! - The graph is a *derived* structure: it can be rebuilt from the live set
//!   at any time, so persistence is a pure open-latency optimization and any
//!   validation failure falls back to exact scan.
//! - Tombstoned slots remain navigable but are never returned, mirroring
//!   pgvector's treatment of dead tuples between DELETE and VACUUM; physical
//!   reclamation happens on compaction followed by a wholesale rebuild.
//! - Inserts are incremental (paper Algorithm 1): no global retraining and
//!   no drift maintenance. Overwriting an existing slot keeps its position
//!   in the graph; distances are always computed against the live vector in
//!   `vectors.bin`, so a stale position only costs recall until the next
//!   compaction-driven rebuild fixes the topology.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::error::{Result, VectorSearchError};
use crate::index::persist::{PersistedHnsw, PersistedNodeRecord};
use crate::storage::vectors::Vectors;
use crate::storage::TombstoneBits;
use crate::types::{DistanceMetric, HnswConfig};

/// Hard cap for generated levels. `ml = 1/ln(m)` makes higher levels
/// exponentially unlikely; the cap only guards pathological RNG draws
/// (pgvector caps by page geometry, which does not apply here).
pub(crate) const MAX_LEVEL: u8 = 32;

/// Minimum connections per layer (> 0), mirroring pgvector `HNSW_MIN_M`.
const MIN_M: usize = 2;

/// Deterministic splitmix64; seeded from the collection name and live count
/// so builds are reproducible across restarts (same property as the IVF
/// k-means seeding).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform sample from (0, 1): 53 mantissa bits, nudged off zero so the
    /// logarithm in the level draw stays finite.
    fn next_open01(&mut self) -> f64 {
        let u = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        if u == 0.0 {
            f64::EPSILON
        } else {
            u
        }
    }
}

/// Entry point of the layered graph.
#[derive(Debug, Clone, Copy)]
struct EntryPoint {
    slot: u32,
    level: u8,
}

/// One graph node. Immutable after creation except for the adjacency lists,
/// which sit behind per-layer locks so searches never block on the store
/// lock (same concurrency shape as the IVF per-list locks).
struct Node {
    level: u8,
    /// Adjacency per layer; one locked list per layer `0..=level`.
    neighbors: Vec<RwLock<Vec<u32>>>,
}

impl Node {
    fn new(level: u8) -> Self {
        Self {
            level,
            neighbors: (0..=level).map(|_| RwLock::new(Vec::new())).collect(),
        }
    }

    /// Adjacency of `lc`. Callers only reach layer `lc` through links created
    /// at that layer, so `lc <= level` holds by construction; the clamp keeps
    /// a corrupted link from panicking.
    fn layer(&self, lc: u8) -> &RwLock<Vec<u32>> {
        &self.neighbors[(lc as usize).min(self.neighbors.len() - 1)]
    }
}

/// Internal (distance, slot) pair ordered by distance for the heaps below.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cand {
    dist: f32,
    slot: u32,
}

impl Eq for Cand {}

impl Ord for Cand {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.dist
            .total_cmp(&other.dist)
            .then(self.slot.cmp(&other.slot))
    }
}

impl PartialOrd for Cand {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// In-memory HNSW index for one collection.
///
/// Mutation paths (insert) run serialized under the store write lock; search
/// paths take short per-node/per-layer locks and an atomic entry-point load,
/// so concurrent searches never block mutations beyond individual list
/// updates.
pub(crate) struct HnswIndex {
    dim: usize,
    metric: DistanceMetric,
    /// Maximum connections per layer above the ground layer.
    m: usize,
    /// Ground-layer cap: `2 * m`, mirroring pgvector `HnswGetLayerM`.
    max_neighbors_0: usize,
    /// Build-time candidate list size.
    ef_construct: usize,
    /// Default layer-0 search width when a query carries none.
    ef_search: usize,
    /// Level assignment factor `1 / ln(m)` from the paper.
    ml: f64,
    /// Slot-indexed node table; `None` marks slots absent from the graph.
    nodes: RwLock<Vec<Option<Arc<Node>>>>,
    entry: RwLock<Option<EntryPoint>>,
    rng: Mutex<SplitMix64>,
    built_at_live_count: AtomicU64,
}

impl HnswIndex {
    fn new(dim: usize, metric: DistanceMetric, config: &HnswConfig, seed: u64) -> Self {
        let m = config.m.max(MIN_M);
        Self {
            dim,
            metric,
            m,
            max_neighbors_0: m * 2,
            ef_construct: config.ef_construct.max(1),
            ef_search: config.ef_search.max(1),
            ml: 1.0 / (m as f64).ln(),
            nodes: RwLock::new(Vec::new()),
            entry: RwLock::new(None),
            rng: Mutex::new(SplitMix64::new(seed)),
            built_at_live_count: AtomicU64::new(0),
        }
    }

    /// Build a fresh index by inserting the given live slots sequentially
    /// (off the store lock; `slots` must be sorted ascending). Sequential
    /// insertion is exactly how pgvector materializes CREATE INDEX.
    pub(crate) fn build(
        config: &HnswConfig,
        collection: &str,
        dim: usize,
        metric: DistanceMetric,
        slots: &[u32],
        vectors: &[Arc<memmap2::Mmap>],
        segment_slots: u32,
    ) -> Result<Self> {
        let mut seed = collection.len() as u64;
        for b in collection.as_bytes() {
            seed = seed.wrapping_mul(0x100_0000_01b3) ^ (*b as u64);
        }
        seed ^= slots.len() as u64;
        seed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);

        let index = Self::new(dim, metric, config, seed);
        for &slot in slots {
            let Some(v) = Vectors::read_slot(vectors, slot as u64, segment_slots, dim) else {
                continue;
            };
            index.insert(slot, v, vectors, segment_slots);
        }
        index
            .built_at_live_count
            .store(slots.len() as u64, Ordering::Relaxed);
        Ok(index)
    }

    /// Rehydrate from persisted state. Structural violations produce
    /// `CorruptData` so callers discard the file and fall back to exact scan.
    pub(crate) fn from_persisted(data: PersistedHnsw, config: &HnswConfig) -> Result<Self> {
        if data.dim == 0 || data.m < MIN_M {
            return Err(VectorSearchError::CorruptData(
                "hnsw.bin invalid header".to_string(),
            ));
        }
        let index = Self::new(
            data.dim,
            data.distance,
            &HnswConfig {
                m: data.m,
                ef_construct: data.ef_construct,
                ..config.clone()
            },
            0,
        );

        let mut nodes = Vec::new();
        for record in &data.nodes {
            if record.slot as usize != nodes.len() {
                return Err(VectorSearchError::CorruptData(
                    "hnsw.bin nodes not dense from slot 0".to_string(),
                ));
            }
            if record.neighbors.len() != record.level as usize + 1 {
                return Err(VectorSearchError::CorruptData(
                    "hnsw.bin node layer count mismatch".to_string(),
                ));
            }
            nodes.push(Some(Arc::new(Node::new(record.level))));
        }

        // Resolve adjacency after all nodes exist.
        for (i, record) in data.nodes.iter().enumerate() {
            let node = nodes[i].as_ref().expect("just pushed");
            for (lc, list) in record.neighbors.iter().enumerate() {
                let cap = index.layer_cap(lc as u8);
                if list.len() > cap || list.iter().any(|&s| s as usize >= nodes.len()) {
                    return Err(VectorSearchError::CorruptData(format!(
                        "hnsw.bin adjacency violation at slot {}",
                        record.slot
                    )));
                }
                *node.layer(lc as u8).write() = list.clone();
            }
        }

        if let Some((slot, level)) = data.entry {
            match nodes.get(slot as usize) {
                Some(Some(node)) if node.level as i32 == level => {}
                _ => {
                    return Err(VectorSearchError::CorruptData(
                        "hnsw.bin entry point invalid".to_string(),
                    ))
                }
            }
            *index.entry.write() = Some(EntryPoint {
                slot,
                level: level as u8,
            });
        }

        *index.nodes.write() = nodes;
        index
            .built_at_live_count
            .store(data.built_at_live_count, Ordering::Relaxed);
        Ok(index)
    }

    /// Snapshot of the persisted representation (for `hnsw.bin`).
    pub(crate) fn to_persisted(&self) -> PersistedHnsw {
        let nodes = self.nodes.read();
        let records = nodes
            .iter()
            .enumerate()
            .filter_map(|(slot, node)| {
                let node = node.as_ref()?;
                Some(PersistedNodeRecord {
                    slot: slot as u32,
                    level: node.level,
                    neighbors: node
                        .neighbors
                        .iter()
                        .map(|l| l.read().clone())
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        PersistedHnsw {
            dim: self.dim,
            distance: self.metric,
            m: self.m,
            ef_construct: self.ef_construct,
            ef_search: self.ef_search,
            entry: (*self.entry.read()).map(|e| (e.slot, e.level as i32)),
            built_at_live_count: self.built_at_live_count.load(Ordering::Relaxed),
            nodes: records,
        }
    }

    pub(crate) fn node_count(&self) -> usize {
        self.nodes.read().iter().flatten().count()
    }

    pub(crate) fn built_at_live_count(&self) -> u64 {
        self.built_at_live_count.load(Ordering::Relaxed)
    }

    pub(crate) fn default_ef(&self) -> usize {
        self.ef_search
    }

    /// Maximum connections per layer above the ground layer.
    pub(crate) fn m(&self) -> usize {
        self.m
    }

    /// Build-time candidate list size.
    pub(crate) fn ef_construct(&self) -> usize {
        self.ef_construct
    }

    fn layer_cap(&self, lc: u8) -> usize {
        if lc == 0 {
            self.max_neighbors_0
        } else {
            self.m
        }
    }

    fn node(&self, slot: u32) -> Option<Arc<Node>> {
        self.nodes.read().get(slot as usize).cloned().flatten()
    }

    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        crate::distance::distance(self.metric, a, b)
    }

    fn random_level(&self) -> u8 {
        let draw = -self.rng.lock().next_open01().ln() * self.ml;
        (draw as u8).min(MAX_LEVEL)
    }

    /// Paper Algorithm 1 (insert). `vector` is the freshly written value of
    /// `slot`; all other node values come from the mmap snapshots.
    ///
    /// Inserting an already-present slot is a no-op: overwrite upserts keep
    /// their graph position (see the module docs for the trade-off).
    pub(crate) fn insert(
        &self,
        slot: u32,
        vector: &[f32],
        vectors: &[Arc<memmap2::Mmap>],
        segment_slots: u32,
    ) {
        if self.node(slot).is_some() {
            return;
        }

        let level = self.random_level();
        let node = Arc::new(Node::new(level));
        {
            let mut nodes = self.nodes.write();
            if slot as usize >= nodes.len() {
                nodes.resize(slot as usize + 1, None);
            }
            nodes[slot as usize] = Some(Arc::clone(&node));
        }

        let Some(entry) = *self.entry.read() else {
            *self.entry.write() = Some(EntryPoint { slot, level });
            return;
        };

        let entry_vec = Vectors::read_slot(vectors, entry.slot as u64, segment_slots, self.dim);
        let mut ep = match entry_vec {
            Some(ev) => vec![Cand {
                dist: self.distance(vector, ev),
                slot: entry.slot,
            }],
            None => return,
        };

        // Phase 1: greedy descent through layers above `level` (ef = 1).
        let mut lc = entry.level;
        while lc > level {
            let Some(&start) = ep.first() else {
                return;
            };
            let best = self.greedy_step(vector, start, lc, vectors, segment_slots);
            ep = vec![best];
            lc -= 1;
        }

        // Phase 2: search + connect on each layer down to the ground.
        for layer in (0..=level.min(entry.level)).rev() {
            let w = self.search_layer(
                vector,
                &ep,
                self.ef_construct,
                layer,
                vectors,
                segment_slots,
                None,
            );
            let selected = self.select_neighbors(&w, self.layer_cap(layer), vectors, segment_slots);
            *node.layer(layer).write() = selected.iter().map(|c| c.slot).collect();
            for cand in &selected {
                self.link_neighbor(cand.slot, slot, cand.dist, layer, vectors, segment_slots);
            }
            ep = w;
        }

        if level > entry.level {
            *self.entry.write() = Some(EntryPoint { slot, level });
        }
    }

    /// Greedy descent with `ef = 1`: repeatedly move to the closest neighbor
    /// until no improvement. Equivalent to running paper Algorithm 2 with
    /// `ef = 1`, which is what pgvector uses between upper layers.
    fn greedy_step(
        &self,
        query: &[f32],
        start: Cand,
        layer: u8,
        vectors: &[Arc<memmap2::Mmap>],
        segment_slots: u32,
    ) -> Cand {
        let mut best = start;
        loop {
            let Some(node) = self.node(best.slot) else {
                return best;
            };
            let neighbors = node.layer(layer).read().clone();
            let mut improved = false;
            for slot in neighbors {
                let Some(v) = Vectors::read_slot(vectors, slot as u64, segment_slots, self.dim)
                else {
                    continue;
                };
                let dist = self.distance(query, v);
                if (Cand { dist, slot }).cmp(&best) == std::cmp::Ordering::Less {
                    best = Cand { dist, slot };
                    improved = true;
                }
            }
            if !improved {
                return best;
            }
        }
    }

    /// Paper Algorithm 2 (SEARCH-LAYER).
    ///
    /// `tombstones` filters which elements count towards `ef` and appear in
    /// the result; tombstoned nodes stay fully traversable so the graph does
    /// not fragment around deletions. `None` (insert-time search) counts
    /// every element, matching pgvector's build behavior.
    #[allow(clippy::too_many_arguments)]
    fn search_layer(
        &self,
        query: &[f32],
        entries: &[Cand],
        ef: usize,
        layer: u8,
        vectors: &[Arc<memmap2::Mmap>],
        segment_slots: u32,
        tombstones: Option<&TombstoneBits>,
    ) -> Vec<Cand> {
        let live = |slot: u32| -> bool {
            match tombstones {
                Some(t) => !t.bit(slot as usize),
                None => true,
            }
        };

        let mut visited: HashSet<u32> = HashSet::with_capacity(entries.len() * 8);
        // Candidate min-heap (closest pop) and result max-heap (furthest pop).
        let mut candidates: std::collections::BinaryHeap<std::cmp::Reverse<Cand>> =
            std::collections::BinaryHeap::new();
        let mut results: std::collections::BinaryHeap<Cand> = std::collections::BinaryHeap::new();
        let mut live_results = 0usize;

        for &cand in entries {
            if !live(cand.slot) || !visited.insert(cand.slot) {
                continue;
            }
            candidates.push(std::cmp::Reverse(cand));
            results.push(cand);
            if live(cand.slot) {
                live_results += 1;
            }
        }

        while let Some(std::cmp::Reverse(c)) = candidates.pop() {
            let Some(furthest) = results.peek() else {
                break;
            };
            if c.dist > furthest.dist && live_results >= ef {
                break;
            }
            let Some(node) = self.node(c.slot) else {
                continue;
            };
            let neighbors = node.layer(layer).read().clone();
            for slot in neighbors {
                if !visited.insert(slot) {
                    continue;
                }
                let Some(v) = Vectors::read_slot(vectors, slot as u64, segment_slots, self.dim)
                else {
                    continue;
                };
                let dist = self.distance(query, v);
                let cand = Cand { dist, slot };
                let Some(furthest) = results.peek() else {
                    continue;
                };
                if live_results < ef || cand.dist < furthest.dist {
                    candidates.push(std::cmp::Reverse(cand));
                    results.push(cand);
                    if live(slot) {
                        live_results += 1;
                        if live_results > ef {
                            // Evict the furthest tracked element.
                            results.pop();
                            live_results -= 1;
                        }
                    }
                }
            }
        }

        // Emit only live elements, closest first.
        let mut out: Vec<Cand> = results.into_iter().filter(|c| live(c.slot)).collect();
        out.sort_unstable_by(|a, b| a.dist.total_cmp(&b.dist));
        out
    }

    /// Paper Algorithm 4 (SELECT-NEIGHBORS-HEURISTIC) with pgvector's
    /// keep-pruned-connections variant: discarded candidates backfill `r`
    /// up to `lm` so nodes never end up under-connected.
    ///
    /// Pairwise distances are measured between candidate vectors, not
    /// against the query.
    fn select_neighbors(
        &self,
        candidates: &[Cand],
        lm: usize,
        vectors: &[Arc<memmap2::Mmap>],
        segment_slots: u32,
    ) -> Vec<Cand> {
        let mut sorted: Vec<Cand> = candidates.to_vec();
        sorted.sort_unstable_by(|a, b| a.dist.total_cmp(&b.dist));
        if sorted.len() <= lm {
            return sorted;
        }

        let mut selected: Vec<Cand> = Vec::with_capacity(lm);
        let mut pruned: Vec<Cand> = Vec::new();
        for cand in sorted {
            if selected.len() == lm {
                break;
            }
            let Some(cv) = Vectors::read_slot(vectors, cand.slot as u64, segment_slots, self.dim)
            else {
                continue;
            };
            let closer = selected.iter().all(|s| {
                let Some(rv) = Vectors::read_slot(vectors, s.slot as u64, segment_slots, self.dim)
                else {
                    return false;
                };
                // Strictly-farther-from-every-selected means diverse enough
                // to keep (pgvector CheckElementCloser).
                self.distance(cv, rv) > cand.dist
            });
            if closer {
                selected.push(cand);
            } else {
                pruned.push(cand);
            }
        }
        for cand in pruned {
            if selected.len() == lm {
                break;
            }
            selected.push(cand);
        }
        selected
    }

    /// Add the reverse edge `neighbor -> slot`, shrinking the neighbor's
    /// list heuristically when it exceeds the layer cap (pgvector
    /// UpdateNeighborOnDisk + HnswUpdateConnection).
    fn link_neighbor(
        &self,
        neighbor: u32,
        slot: u32,
        dist_to_neighbor: f32,
        layer: u8,
        vectors: &[Arc<memmap2::Mmap>],
        segment_slots: u32,
    ) {
        let Some(node) = self.node(neighbor) else {
            return;
        };
        let lm = self.layer_cap(layer);
        let mut adj = node.layer(layer).write();
        if adj.contains(&slot) {
            return;
        }
        if adj.len() < lm {
            adj.push(slot);
            return;
        }

        // Overflow: reselect around the neighbor's own vector.
        let Some(nv) = Vectors::read_slot(vectors, neighbor as u64, segment_slots, self.dim) else {
            return;
        };
        let mut pool: Vec<Cand> = adj
            .iter()
            .filter_map(|&s| {
                let v = Vectors::read_slot(vectors, s as u64, segment_slots, self.dim)?;
                Some(Cand {
                    dist: self.distance(nv, v),
                    slot: s,
                })
            })
            .collect();
        pool.push(Cand {
            dist: dist_to_neighbor,
            slot,
        });
        let selected = self.select_neighbors(&pool, lm, vectors, segment_slots);
        *adj = selected.into_iter().map(|c| c.slot).collect();
    }

    /// Approximate kNN: greedy descent to layer 1, full search at layer 0.
    /// Returns `(score, slot)` pairs in output-score space (higher is
    /// better), unsorted; shared post-processing stays in `CollectionStore`.
    pub(crate) fn probe_candidates(
        &self,
        query: &[f32],
        ef: usize,
        k: usize,
        tombstones: &TombstoneBits,
        vectors: &[Arc<memmap2::Mmap>],
        segment_slots: u32,
    ) -> Result<Vec<(f32, u32)>> {
        let Some(entry) = *self.entry.read() else {
            return Ok(Vec::new());
        };
        let Some(entry_vec) =
            Vectors::read_slot(vectors, entry.slot as u64, segment_slots, self.dim)
        else {
            return Ok(Vec::new());
        };

        let mut best = Cand {
            dist: self.distance(query, entry_vec),
            slot: entry.slot,
        };
        let mut lc = entry.level;
        while lc > 0 {
            best = self.greedy_step(query, best, lc, vectors, segment_slots);
            lc -= 1;
        }

        let w = self.search_layer(
            query,
            &[best],
            ef.max(k),
            0,
            vectors,
            segment_slots,
            Some(tombstones),
        );
        Ok(w.into_iter()
            .map(|c| (crate::distance::to_score(self.metric, c.dist), c.slot))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DistanceMetric;

    const DIM: usize = 8;

    fn config() -> HnswConfig {
        HnswConfig {
            m: 8,
            ef_construct: 32,
            ef_search: 24,
            ..HnswConfig::default()
        }
    }

    /// Two separated blobs of 8-dim vectors.
    fn blobs(n_per_blob: usize) -> Vec<(Vec<f32>, usize)> {
        let mut out: Vec<(Vec<f32>, usize)> = Vec::new();
        for (blob, center) in [[0.0f32; DIM], [50.0; DIM]].iter().enumerate() {
            for i in 0..n_per_blob {
                let mut v = *center;
                v[i % DIM] += i as f32 * 0.1;
                out.push((v.to_vec(), blob));
            }
        }
        out
    }

    fn mmap_from(vectors: &[Vec<f32>]) -> (tempfile::TempDir, Vec<Arc<memmap2::Mmap>>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vectors.bin");
        let mut bytes = Vec::with_capacity(vectors.len() * DIM * 4);
        for v in vectors {
            for x in v {
                bytes.extend_from_slice(&x.to_le_bytes());
            }
        }
        std::fs::write(&path, &bytes).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let mmap = unsafe { memmap2::Mmap::map(&file).unwrap() };
        (dir, vec![Arc::new(mmap)])
    }

    fn all_live(n: usize) -> TombstoneBits {
        TombstoneBits::from_bits(bitvec::bitvec![0; n])
    }

    #[test]
    fn test_insert_and_probe_finds_blobs() {
        let data = blobs(40);
        let vectors: Vec<Vec<f32>> = data.iter().map(|(v, _)| v.clone()).collect();
        let (_dir, mmaps) = mmap_from(&vectors);
        let slots: Vec<u32> = (0..data.len() as u32).collect();

        let index = HnswIndex::build(
            &config(),
            "col",
            DIM,
            DistanceMetric::Euclid,
            &slots,
            &mmaps,
            128,
        )
        .unwrap();

        assert_eq!(index.node_count(), data.len());

        // Every query's top-10 must come from its own blob: the blobs sit
        // 50 units apart while members stay within ~1 unit of their center,
        // so cross-blob hits in the top-10 would indicate broken navigation.
        for (blob, center) in [[0.0f32; DIM], [50.0; DIM]].iter().enumerate() {
            let q = *center;
            let mut hits = index
                .probe_candidates(&q, 24, 10, &all_live(data.len()), &mmaps, 128)
                .unwrap();
            assert!(hits.len() >= 10, "expected results");
            hits.sort_by(|a, b| b.0.total_cmp(&a.0));
            for &(_, slot) in hits.iter().take(10) {
                assert_eq!(
                    data[slot as usize].1, blob,
                    "slot {slot} from the wrong blob"
                );
            }
        }
    }

    #[test]
    fn test_probe_respects_tombstones() {
        let data = blobs(20);
        let vectors: Vec<Vec<f32>> = data.iter().map(|(v, _)| v.clone()).collect();
        let (_dir, mmaps) = mmap_from(&vectors);
        let slots: Vec<u32> = (0..data.len() as u32).collect();
        let index = HnswIndex::build(
            &config(),
            "col",
            DIM,
            DistanceMetric::Euclid,
            &slots,
            &mmaps,
            128,
        )
        .unwrap();

        let mut bits = bitvec::bitvec![0; data.len()];
        bits.set(3, true);
        let tombstones = TombstoneBits::from_bits(bits);
        let hits = index
            .probe_candidates(&[50.0; DIM], 64, 100, &tombstones, &mmaps, 128)
            .unwrap();
        assert!(!hits.iter().any(|&(_, s)| s == 3));

        // The tombstoned node stays navigable: dropping the filter brings it
        // back without any structural repair.
        let hits = index
            .probe_candidates(&[50.0; DIM], 64, 100, &all_live(data.len()), &mmaps, 128)
            .unwrap();
        assert!(hits.iter().any(|&(_, s)| s == 3));
    }

    #[test]
    fn test_incremental_insert_reaches_graph() {
        let data = blobs(20);
        let mut vectors: Vec<Vec<f32>> = data.iter().map(|(v, _)| v.clone()).collect();
        let (_dir, mmaps) = mmap_from(&vectors);
        let slots: Vec<u32> = (0..data.len() as u32).collect();
        let index = HnswIndex::build(
            &config(),
            "col",
            DIM,
            DistanceMetric::Euclid,
            &slots,
            &mmaps,
            256,
        )
        .unwrap();

        // Insert one extra far-away point through the incremental path.
        vectors.push(vec![-100.0; DIM]);
        let (_dir2, mmaps2) = mmap_from(&vectors);
        let new_slot = data.len() as u32;
        let v = vec![-100.0f32; DIM];
        index.insert(new_slot, &v, &mmaps2, 256);
        assert_eq!(index.node_count(), data.len() + 1);

        let hits = index
            .probe_candidates(
                &[-100.0; DIM],
                64,
                5,
                &all_live(vectors.len()),
                &mmaps2,
                256,
            )
            .unwrap();
        assert!(
            hits.iter().any(|&(_, s)| s == new_slot),
            "incrementally inserted point must be reachable"
        );
    }

    #[test]
    fn test_overwrite_insert_is_noop() {
        let data = blobs(6);
        let vectors: Vec<Vec<f32>> = data.iter().map(|(v, _)| v.clone()).collect();
        let (_dir, mmaps) = mmap_from(&vectors);
        let slots: Vec<u32> = (0..data.len() as u32).collect();
        let index = HnswIndex::build(
            &config(),
            "col",
            DIM,
            DistanceMetric::Euclid,
            &slots,
            &mmaps,
            64,
        )
        .unwrap();
        let before = index.node_count();
        index.insert(0, &vectors[0], &mmaps, 64);
        assert_eq!(index.node_count(), before);
    }

    #[test]
    fn test_persisted_roundtrip_preserves_topology() {
        let data = blobs(30);
        let vectors: Vec<Vec<f32>> = data.iter().map(|(v, _)| v.clone()).collect();
        let (_dir, mmaps) = mmap_from(&vectors);
        let slots: Vec<u32> = (0..data.len() as u32).collect();
        let original = HnswIndex::build(
            &config(),
            "col",
            DIM,
            DistanceMetric::Euclid,
            &slots,
            &mmaps,
            128,
        )
        .unwrap();

        let restored = HnswIndex::from_persisted(original.to_persisted(), &config()).unwrap();
        assert_eq!(restored.node_count(), original.node_count());
        assert_eq!(
            restored.built_at_live_count(),
            original.built_at_live_count()
        );
        assert_eq!(
            restored.default_ef(),
            original.default_ef(),
            "effective ef_search must survive the roundtrip"
        );

        let q = vec![50.0; DIM];
        let a = original
            .probe_candidates(&q, 32, 10, &all_live(data.len()), &mmaps, 128)
            .unwrap();
        let b = restored
            .probe_candidates(&q, 32, 10, &all_live(data.len()), &mmaps, 128)
            .unwrap();
        assert_eq!(a, b, "restored graph must answer identically");
    }

    #[test]
    fn test_from_persisted_rejects_corruption() {
        let data = blobs(10);
        let vectors: Vec<Vec<f32>> = data.iter().map(|(v, _)| v.clone()).collect();
        let (_dir, mmaps) = mmap_from(&vectors);
        let slots: Vec<u32> = (0..data.len() as u32).collect();
        let index = HnswIndex::build(
            &config(),
            "col",
            DIM,
            DistanceMetric::Euclid,
            &slots,
            &mmaps,
            64,
        )
        .unwrap();

        let mut persisted = index.to_persisted();
        persisted.nodes[2].neighbors[0][0] = 999;
        assert!(HnswIndex::from_persisted(persisted, &config()).is_err());

        let mut persisted = index.to_persisted();
        persisted.entry = Some((999, 0));
        assert!(HnswIndex::from_persisted(persisted, &config()).is_err());

        let mut persisted = index.to_persisted();
        persisted.nodes[1].level = 5;
        assert!(HnswIndex::from_persisted(persisted, &config()).is_err());
    }

    #[test]
    fn test_recall_against_exact_scan() {
        // Random-ish deterministic vectors; HNSW with these settings should
        // recover most of the true top-10 for queries near the data cloud.
        let n = 400;
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(n);
        let mut state = SplitMix64::new(42);
        for _ in 0..n {
            vectors.push(
                (0..DIM)
                    .map(|_| (state.next_u64() % 1000) as f32 / 100.0)
                    .collect(),
            );
        }
        let (_dir, mmaps) = mmap_from(&vectors);
        let slots: Vec<u32> = (0..n as u32).collect();
        let index = HnswIndex::build(
            &config(),
            "col",
            DIM,
            DistanceMetric::Euclid,
            &slots,
            &mmaps,
            1024,
        )
        .unwrap();

        let q = vec![5.0f32; DIM];
        let mut exact: Vec<(f32, u32)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                (
                    crate::distance::distance(DistanceMetric::Euclid, &q, v),
                    i as u32,
                )
            })
            .collect();
        exact.sort_by(|a, b| a.0.total_cmp(&b.0));
        let truth: HashSet<u32> = exact[..10].iter().map(|&(_, s)| s).collect();

        let hits = index
            .probe_candidates(&q, 48, 10, &all_live(n), &mmaps, 1024)
            .unwrap();
        let found = hits.iter().filter(|&&(_, s)| truth.contains(&s)).count();
        assert!(found >= 7, "recall@10 too low: {found}/10");
    }
}
