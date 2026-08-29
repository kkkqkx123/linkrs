//! Calibrator Tree for Density Balancing
//!
//! Hierarchical aggregation of per-region density metrics to dynamically
//! adjust compaction thresholds based on workload characteristics and
//! memory pressure.

use graphdb_core::types::Timestamp;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calibrated density threshold for compaction decisions.
#[derive(Debug, Clone, Copy)]
pub struct CalibratedThreshold {
    pub base_deletion_ratio: f64,
    pub base_fragmentation_ratio: f32,
    pub multiplier: f64,
}

impl CalibratedThreshold {
    pub fn effective_deletion_ratio(&self) -> f64 {
        (self.base_deletion_ratio * self.multiplier).clamp(0.05, 0.95)
    }

    pub fn effective_fragmentation_ratio(&self) -> f32 {
        ((self.base_fragmentation_ratio as f64) * self.multiplier) as f32
    }
}

/// Per-node statistics in the calibrator tree.
#[derive(Debug, Clone, Default)]
pub struct DensityStats {
    pub edge_count: u64,
    pub deleted_count: u64,
    pub fragmented_capacity: u64,
    pub access_count: u64,
    pub last_compact_ts: Timestamp,
}

impl DensityStats {
    pub fn deletion_ratio(&self) -> f64 {
        if self.edge_count == 0 {
            0.0
        } else {
            self.deleted_count as f64 / self.edge_count as f64
        }
    }

    pub fn space_efficiency(&self) -> f64 {
        if self.edge_count == 0 {
            1.0
        } else if self.fragmented_capacity == 0 {
            1.0
        } else {
            1.0 - (self.fragmented_capacity as f64
                / (self.edge_count + self.fragmented_capacity) as f64)
        }
    }

    /// Merge two stats for parent node aggregation.
    pub fn merge(&mut self, other: &DensityStats) {
        self.edge_count += other.edge_count;
        self.deleted_count += other.deleted_count;
        self.fragmented_capacity += other.fragmented_capacity;
        self.access_count += other.access_count;
        self.last_compact_ts = self.last_compact_ts.max(other.last_compact_ts);
    }
}

/// Configuration for the calibrator tree.
#[derive(Debug, Clone)]
pub struct CalibratorConfig {
    /// Base deletion ratio threshold (default: 0.5).
    pub base_deletion_ratio: f64,
    /// Base fragmentation threshold (default: 2.0).
    pub base_fragmentation_ratio: f32,
    /// Memory pressure trigger (buffer pool usage ratio).
    pub memory_pressure_threshold: f64,
    /// Minimum multiplier (most aggressive compactions).
    pub min_multiplier: f64,
    /// Maximum multiplier (least aggressive compactions).
    pub max_multiplier: f64,
    /// Branching factor for internal tree nodes.
    pub branch_factor: usize,
}

impl Default for CalibratorConfig {
    fn default() -> Self {
        Self {
            base_deletion_ratio: 0.5,
            base_fragmentation_ratio: 2.0,
            memory_pressure_threshold: 0.8,
            min_multiplier: 0.5,
            max_multiplier: 2.0,
            branch_factor: 4,
        }
    }
}

/// Calibrator tree node.
pub struct CalibratorNode {
    pub stats: DensityStats,
    pub children: Vec<usize>,
    pub region_ids: Vec<u32>,
    pub access_count: AtomicU64,
}

impl Clone for CalibratorNode {
    fn clone(&self) -> Self {
        Self {
            stats: self.stats.clone(),
            children: self.children.clone(),
            region_ids: self.region_ids.clone(),
            access_count: AtomicU64::new(self.access_count.load(Ordering::Relaxed)),
        }
    }
}

impl std::fmt::Debug for CalibratorNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalibratorNode")
            .field("stats", &self.stats)
            .field("children", &self.children)
            .field("region_ids", &self.region_ids)
            .field("access_count", &self.access_count.load(Ordering::Relaxed))
            .finish()
    }
}

impl CalibratorNode {
    fn new_leaf(region_id: u32) -> Self {
        Self {
            stats: DensityStats::default(),
            children: Vec::new(),
            region_ids: vec![region_id],
            access_count: AtomicU64::new(0),
        }
    }

    fn new_internal(children: Vec<usize>, region_ids: Vec<u32>) -> Self {
        Self {
            stats: DensityStats::default(),
            children,
            region_ids,
            access_count: AtomicU64::new(0),
        }
    }

    fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// Calibrator tree for density-based compaction calibration.
pub struct CalibratorTree {
    nodes: Vec<CalibratorNode>,
    root_idx: usize,
    config: CalibratorConfig,
    leaf_index: Vec<Option<usize>>,
    memory_pressure: f64,
}

impl std::fmt::Debug for CalibratorTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalibratorTree")
            .field("nodes", &self.nodes)
            .field("root_idx", &self.root_idx)
            .field("config", &self.config)
            .field("memory_pressure", &self.memory_pressure)
            .finish()
    }
}

impl Clone for CalibratorTree {
    fn clone(&self) -> Self {
        Self {
            nodes: self.nodes.clone(),
            root_idx: self.root_idx,
            config: self.config.clone(),
            leaf_index: self.leaf_index.clone(),
            memory_pressure: self.memory_pressure,
        }
    }
}

impl CalibratorTree {
    pub fn new(config: CalibratorConfig) -> Self {
        let root = CalibratorNode {
            stats: DensityStats::default(),
            children: Vec::new(),
            region_ids: Vec::new(),
            access_count: AtomicU64::new(0),
        };
        Self {
            nodes: vec![root],
            root_idx: 0,
            config,
            leaf_index: Vec::new(),
            memory_pressure: 0.0,
        }
    }

    pub fn with_region_count(region_count: usize, config: CalibratorConfig) -> Self {
        let mut tree = Self::new(config);
        if region_count > 0 {
            tree.ensure_region_count(region_count);
        }
        tree
    }

    pub fn config(&self) -> &CalibratorConfig {
        &self.config
    }

    pub fn region_count(&self) -> usize {
        self.leaf_index.len()
    }

    /// Ensure the tree can address `region_count` regions, rebuilding if needed.
    pub fn ensure_region_count(&mut self, region_count: usize) {
        if region_count == self.leaf_index.len() {
            return;
        }
        self.rebuild_tree(region_count);
    }

    fn rebuild_tree(&mut self, region_count: usize) {
        if region_count == 0 {
            let root = CalibratorNode {
                stats: DensityStats::default(),
                children: Vec::new(),
                region_ids: Vec::new(),
                access_count: AtomicU64::new(0),
            };
            self.nodes = vec![root];
            self.root_idx = 0;
            self.leaf_index = Vec::new();
            return;
        }

        let branch_factor = self.config.branch_factor.max(2);
        let mut nodes: Vec<CalibratorNode> = Vec::new();
        let mut leaf_index: Vec<Option<usize>> = vec![None; region_count];

        for rid in 0..region_count as u32 {
            let idx = nodes.len();
            nodes.push(CalibratorNode::new_leaf(rid));
            leaf_index[rid as usize] = Some(idx);
        }

        let mut current_level: Vec<usize> = (0..region_count).collect();
        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(branch_factor) {
                let children = chunk.to_vec();
                let mut region_ids = Vec::new();
                for &child_idx in &children {
                    region_ids.extend_from_slice(&nodes[child_idx].region_ids);
                }
                let parent_idx = nodes.len();
                nodes.push(CalibratorNode::new_internal(children, region_ids));
                next_level.push(parent_idx);
            }
            current_level = next_level;
        }

        let root_idx = if nodes.is_empty() {
            0
        } else {
            current_level.first().copied().unwrap_or(0)
        };

        if nodes.is_empty() {
            nodes.push(CalibratorNode {
                stats: DensityStats::default(),
                children: Vec::new(),
                region_ids: Vec::new(),
                access_count: AtomicU64::new(0),
            });
        }

        self.nodes = nodes;
        self.root_idx = root_idx;
        self.leaf_index = leaf_index;
        self.recompute_aggregates();
    }

    fn recompute_aggregates(&mut self) {
        // Bottom-up aggregation: leaves already have stats, internal nodes recompute from children.
        // Since nodes were created leaf-first then parents, children indices are always < parent index.
        for idx in 0..self.nodes.len() {
            if self.nodes[idx].is_leaf() {
                continue;
            }
            let children = self.nodes[idx].children.clone();
            let mut agg = DensityStats::default();
            let mut total_access = 0u64;
            for &child_idx in &children {
                agg.merge(&self.nodes[child_idx].stats);
                total_access += self.nodes[child_idx].access_count.load(Ordering::Relaxed);
            }
            // Fold access_count into stats.access_count for threshold decisions as well
            agg.access_count = total_access;
            self.nodes[idx].stats = agg;
            self.nodes[idx]
                .access_count
                .store(total_access, Ordering::Relaxed);
        }
    }

    /// Update statistics for a single region.
    pub fn update_region_stats(&mut self, region_id: u32, stats: DensityStats) {
        let rid = region_id as usize;
        if rid >= self.leaf_index.len() {
            self.ensure_region_count(rid + 1);
        }
        let leaf_idx = match self.leaf_index.get(rid).and_then(|o| *o) {
            Some(idx) => idx,
            None => return,
        };
        let access = self.nodes[leaf_idx].access_count.load(Ordering::Relaxed);
        let mut new_stats = stats;
        new_stats.access_count = access;
        self.nodes[leaf_idx].stats = new_stats;
        self.propagate_to_root(leaf_idx);
    }

    fn propagate_to_root(&mut self, leaf_idx: usize) {
        // Find all ancestors of leaf_idx by scanning nodes for those containing leaf_idx in children.
        // Since tree is shallow and region_count moderate, linear scan is acceptable.
        // We do bottom-up: for each level from leaf parent up to root, recompute.
        let mut current = leaf_idx;
        loop {
            let parent = self.find_parent(current);
            match parent {
                Some(pidx) => {
                    let children = self.nodes[pidx].children.clone();
                    let mut agg = DensityStats::default();
                    let mut total_access = 0u64;
                    for &c in &children {
                        agg.merge(&self.nodes[c].stats);
                        total_access += self.nodes[c].access_count.load(Ordering::Relaxed);
                    }
                    agg.access_count = total_access;
                    self.nodes[pidx].stats = agg;
                    self.nodes[pidx]
                        .access_count
                        .store(total_access, Ordering::Relaxed);
                    current = pidx;
                    if current == self.root_idx {
                        break;
                    }
                }
                None => break,
            }
        }
    }

    fn find_parent(&self, child_idx: usize) -> Option<usize> {
        for (idx, node) in self.nodes.iter().enumerate() {
            if node.children.contains(&child_idx) {
                return Some(idx);
            }
        }
        None
    }

    /// Record an access for hot/cold classification.
    pub fn record_access(&self, region_id: u32) {
        let rid = region_id as usize;
        if rid >= self.leaf_index.len() {
            return;
        }
        if let Some(Some(leaf_idx)) = self.leaf_index.get(rid) {
            self.nodes[*leaf_idx]
                .access_count
                .fetch_add(1, Ordering::Relaxed);
            // Also update root's aggregated counter for global hot detection.
            self.nodes[self.root_idx]
                .access_count
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get stats for a specific region.
    pub fn region_stats(&self, region_id: u32) -> Option<DensityStats> {
        let rid = region_id as usize;
        let leaf_idx = self.leaf_index.get(rid)?.as_ref()?;
        let mut s = self.nodes[*leaf_idx].stats.clone();
        s.access_count = self.nodes[*leaf_idx].access_count.load(Ordering::Relaxed);
        Some(s)
    }

    /// Global aggregated stats (root node).
    pub fn global_stats(&self) -> DensityStats {
        let mut s = self.nodes[self.root_idx].stats.clone();
        s.access_count = self.nodes[self.root_idx]
            .access_count
            .load(Ordering::Relaxed);
        s
    }

    /// Calibrated threshold based on current global metrics.
    pub fn calibrated_threshold(&self) -> CalibratedThreshold {
        let root_stats = self.global_stats();
        let mut multiplier = 1.0;

        if self.memory_pressure > self.config.memory_pressure_threshold {
            let range = 1.0 - self.config.memory_pressure_threshold;
            let excess = if range > f64::EPSILON {
                (self.memory_pressure - self.config.memory_pressure_threshold) / range
            } else {
                1.0
            };
            multiplier *= 1.0 - excess * 0.5;
        }

        let del_ratio = root_stats.deletion_ratio();
        if del_ratio > 0.3 {
            multiplier *= 0.8;
        } else if del_ratio > 0.15 {
            multiplier *= 0.9;
        } else if del_ratio < 0.05 {
            multiplier *= 1.1;
        }

        let hot_ratio = if root_stats.edge_count > 0 {
            root_stats.access_count as f64 / root_stats.edge_count as f64
        } else {
            0.0
        };
        if hot_ratio > 10.0 {
            multiplier *= 0.95;
        }

        multiplier = multiplier.clamp(self.config.min_multiplier, self.config.max_multiplier);

        CalibratedThreshold {
            base_deletion_ratio: self.config.base_deletion_ratio,
            base_fragmentation_ratio: self.config.base_fragmentation_ratio,
            multiplier,
        }
    }

    /// Determine if a specific region should be compacted under calibrated threshold.
    pub fn should_compact_region(&self, region_id: u32) -> bool {
        if let Some(stats) = self.region_stats(region_id) {
            let threshold = self.calibrated_threshold();
            let del_ratio = stats.deletion_ratio();
            if del_ratio >= threshold.effective_deletion_ratio() {
                return true;
            }
            // Consider fragmentation: fragmented_capacity as wasted slots ratio
            let frag_ratio = if stats.edge_count > 0 {
                (stats.edge_count + stats.fragmented_capacity) as f32 / stats.edge_count as f32
            } else {
                0.0
            };
            if frag_ratio >= threshold.effective_fragmentation_ratio() {
                return true;
            }
        }
        false
    }

    /// Check if compaction should be triggered globally.
    pub fn should_trigger_compaction(&self, total_edges: u64, total_tombstones: u64) -> bool {
        if total_edges == 0 {
            return false;
        }
        let ratio = total_tombstones as f64 / total_edges as f64;
        ratio >= self.calibrated_threshold().effective_deletion_ratio()
    }

    pub fn set_memory_pressure(&mut self, ratio: f64) {
        self.memory_pressure = ratio.clamp(0.0, 1.0);
    }

    /// Whether a region is hot (frequently accessed).
    pub fn is_hot_region(&self, region_id: u32, threshold: u64) -> bool {
        let rid = region_id as usize;
        if let Some(Some(leaf_idx)) = self.leaf_index.get(rid) {
            self.nodes[*leaf_idx].access_count.load(Ordering::Relaxed) >= threshold
        } else {
            false
        }
    }
}

impl Default for CalibratorTree {
    fn default() -> Self {
        Self::new(CalibratorConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_density_stats_merge() {
        let mut a = DensityStats {
            edge_count: 100,
            deleted_count: 10,
            fragmented_capacity: 20,
            access_count: 5,
            last_compact_ts: 100,
        };
        let b = DensityStats {
            edge_count: 50,
            deleted_count: 5,
            fragmented_capacity: 10,
            access_count: 3,
            last_compact_ts: 200,
        };
        a.merge(&b);
        assert_eq!(a.edge_count, 150);
        assert_eq!(a.deleted_count, 15);
        assert_eq!(a.fragmented_capacity, 30);
        assert_eq!(a.access_count, 8);
        assert_eq!(a.last_compact_ts, 200);
        assert!((a.deletion_ratio() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn test_calibrated_threshold_memory_pressure() {
        let config = CalibratorConfig {
            base_deletion_ratio: 0.5,
            base_fragmentation_ratio: 2.0,
            memory_pressure_threshold: 0.8,
            min_multiplier: 0.5,
            max_multiplier: 2.0,
            branch_factor: 4,
        };
        let mut tree = CalibratorTree::new(config);
        let base = tree.calibrated_threshold();
        let base_ratio = base.effective_deletion_ratio();

        tree.set_memory_pressure(0.95);
        let high_mem = tree.calibrated_threshold();
        // High memory pressure should lower threshold (more aggressive)
        assert!(high_mem.effective_deletion_ratio() < base_ratio);
        assert!(high_mem.multiplier < base.multiplier);
    }

    #[test]
    fn test_calibrator_region_update_and_global() {
        let config = CalibratorConfig::default();
        let mut tree = CalibratorTree::with_region_count(8, config);
        assert_eq!(tree.region_count(), 8);

        for rid in 0..8u32 {
            tree.update_region_stats(
                rid,
                DensityStats {
                    edge_count: 100,
                    deleted_count: 10,
                    fragmented_capacity: 5,
                    access_count: 0,
                    last_compact_ts: 0,
                },
            );
        }
        let global = tree.global_stats();
        assert_eq!(global.edge_count, 800);
        assert_eq!(global.deleted_count, 80);
        assert!((global.deletion_ratio() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn test_record_access_and_hot() {
        let config = CalibratorConfig::default();
        let tree = CalibratorTree::with_region_count(4, config);
        tree.record_access(1);
        tree.record_access(1);
        tree.record_access(1);
        assert!(tree.is_hot_region(1, 3));
        assert!(!tree.is_hot_region(1, 4));
        assert!(!tree.is_hot_region(2, 1));
    }

    #[test]
    fn test_should_compact_region() {
        let config = CalibratorConfig {
            base_deletion_ratio: 0.5,
            ..Default::default()
        };
        let mut tree = CalibratorTree::with_region_count(2, config);
        tree.update_region_stats(
            0,
            DensityStats {
                edge_count: 100,
                deleted_count: 60,
                fragmented_capacity: 0,
                access_count: 0,
                last_compact_ts: 0,
            },
        );
        // 60% deletion > 50% base => should compact
        assert!(tree.should_compact_region(0));
        tree.update_region_stats(
            1,
            DensityStats {
                edge_count: 100,
                deleted_count: 10,
                fragmented_capacity: 0,
                access_count: 0,
                last_compact_ts: 0,
            },
        );
        assert!(!tree.should_compact_region(1));
    }

    #[test]
    fn test_tree_rebuild_on_growth() {
        let config = CalibratorConfig::default();
        let mut tree = CalibratorTree::with_region_count(2, config);
        assert_eq!(tree.region_count(), 2);
        tree.update_region_stats(
            5,
            DensityStats {
                edge_count: 10,
                ..Default::default()
            },
        );
        assert!(tree.region_count() >= 6);
        assert_eq!(tree.region_stats(5).unwrap().edge_count, 10);
    }

    #[test]
    fn test_multiplier_clamping() {
        let config = CalibratorConfig {
            base_deletion_ratio: 0.5,
            base_fragmentation_ratio: 2.0,
            memory_pressure_threshold: 0.0,
            min_multiplier: 0.7,
            max_multiplier: 1.3,
            branch_factor: 4,
        };
        let mut tree = CalibratorTree::new(config);
        tree.set_memory_pressure(1.0);
        let t = tree.calibrated_threshold();
        assert!(t.multiplier >= 0.7 && t.multiplier <= 1.3);
    }
}
