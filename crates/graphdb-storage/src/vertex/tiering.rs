//! Cold-hot tiering decision gate.
//!
//! Evidence first, then exactly one branch: hot concentration walks the
//! tiering branch (per-key estimates, cold-shape choice, background
//! promotion), uniform traffic walks the degraded branch (deployment
//! budget, over-limit write refusal, patrol water-level alarm). The two
//! branches are mutually exclusive by construction: the gate returns one
//! variant, never both. Neither branch changes id encoding, shard routing,
//! delete-reuse semantics, or lineage marks; tiering stays a per-shard
//! implementation detail.

/// Which branch the sampled evidence selects. Mutually exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieringBranch {
    /// Too little traffic to conclude; keep sampling, build nothing.
    NeedMoreEvidence,
    /// Few keys draw most probes: build the tiering branch.
    Tier,
    /// Uniform traffic: tiering would add a hop with no memory win, run
    /// the budget guard instead.
    BudgetGuard,
}

/// Sampled evidence feeding the gate.
#[derive(Debug, Clone, Copy)]
pub struct TieringEvidence {
    /// Total key probes served across shards.
    pub total_probes: u64,
    /// Hottest single shard's probe count (concentration signal).
    pub max_shard_probes: u64,
    /// Share of probes drawn by the estimated hot set (0.0..=1.0).
    pub hot_share: f64,
}

/// Minimum probes before the gate may conclude anything.
pub const TIERING_MIN_PROBES: u64 = 100_000;

/// Hottest-shard share at or above which traffic counts as concentrated.
pub const TIERING_SKEW_THRESHOLD: f64 = 0.5;

/// Hot-set probe share at or above which promotion pays off.
pub const TIERING_HOT_SHARE_THRESHOLD: f64 = 0.8;

/// Evidence-first gate: below the probe floor there is no conclusion;
/// concentrated traffic selects tiering, everything else selects the
/// budget guard.
pub fn decide_branch(evidence: TieringEvidence) -> TieringBranch {
    if evidence.total_probes < TIERING_MIN_PROBES {
        return TieringBranch::NeedMoreEvidence;
    }
    let skew = if evidence.total_probes == 0 {
        0.0
    } else {
        evidence.max_shard_probes as f64 / evidence.total_probes as f64
    };
    if skew >= TIERING_SKEW_THRESHOLD && evidence.hot_share >= TIERING_HOT_SHARE_THRESHOLD {
        TieringBranch::Tier
    } else {
        TieringBranch::BudgetGuard
    }
}

/// Cold-index shape candidates scored on point-lookup P99, memory cap,
/// and implementation complexity. Benchmark caliber reuses the gate
/// groups (build, point lookup, scan, churn).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdIndexChoice {
    /// Sharded persistent hash in the ladybug slot style.
    PersistentHash,
    /// Memory-mapped ordered index riding the page cache.
    MmapOrdered,
}

/// Three-factor cold-shape score: lower is better. P99 is normalized by
/// the hard budget, memory by the cap, complexity is a 0..=1 reviewer
/// score. Exactly one winner, no ties escape (hash wins ties).
pub fn score_cold_shape(
    p99_ns: u64,
    p99_budget_ns: u64,
    memory_bytes: u64,
    memory_cap_bytes: u64,
    hash_complexity: f64,
    mmap_complexity: f64,
) -> ColdIndexChoice {
    let score = |p99: u64, mem: u64, complexity: f64| {
        let latency = p99 as f64 / p99_budget_ns.max(1) as f64;
        let memory = mem as f64 / memory_cap_bytes.max(1) as f64;
        latency + memory + complexity.clamp(0.0, 1.0)
    };
    let hash = score(p99_ns, memory_bytes, hash_complexity);
    // Ordered scans trade lookup latency for scan throughput; model that
    // as a fixed scan credit so the choice stays two-way and explainable.
    let mmap = score(p99_ns, memory_bytes, mmap_complexity) - 0.1;
    if hash <= mmap {
        ColdIndexChoice::PersistentHash
    } else {
        ColdIndexChoice::MmapOrdered
    }
}

/// Deployment-given primary-key heap budget for the degraded branch.
/// Thresholds come from configuration, never hardcoded at call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PkIndexBudget {
    /// Maximum resident primary-key heap bytes across shards.
    pub max_bytes: usize,
}

impl PkIndexBudget {
    /// Usage ratio 0.0..=1.0+ for the patrol water-level alarm.
    pub fn pressure_ratio(&self, used_bytes: usize) -> f64 {
        if self.max_bytes == 0 {
            return 0.0;
        }
        used_bytes as f64 / self.max_bytes as f64
    }

    /// Over-limit check for the write entry: refuses the write with the
    /// current usage in the message instead of growing past budget.
    pub fn check(&self, used_bytes: usize) -> graphdb_core::StorageResult<()> {
        if used_bytes > self.max_bytes {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "primary-key index heap {used_bytes} bytes exceeds budget {} bytes; \
                 refusing over-limit write",
                self.max_bytes,
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_needs_evidence_first() {
        assert_eq!(
            decide_branch(TieringEvidence {
                total_probes: 10,
                max_shard_probes: 10,
                hot_share: 1.0,
            }),
            TieringBranch::NeedMoreEvidence
        );
    }

    #[test]
    fn test_concentrated_traffic_selects_tiering() {
        assert_eq!(
            decide_branch(TieringEvidence {
                total_probes: 200_000,
                max_shard_probes: 150_000,
                hot_share: 0.9,
            }),
            TieringBranch::Tier
        );
    }

    #[test]
    fn test_uniform_traffic_selects_budget_guard() {
        assert_eq!(
            decide_branch(TieringEvidence {
                total_probes: 200_000,
                max_shard_probes: 30_000,
                hot_share: 0.2,
            }),
            TieringBranch::BudgetGuard
        );
        assert_eq!(
            decide_branch(TieringEvidence {
                total_probes: 200_000,
                max_shard_probes: 150_000,
                hot_share: 0.3,
            }),
            TieringBranch::BudgetGuard
        );
    }

    #[test]
    fn test_cold_shape_scores_pick_one() {
        let choice = score_cold_shape(100, 1000, 100, 10_000, 0.2, 0.9);
        assert_eq!(choice, ColdIndexChoice::PersistentHash);
        let choice = score_cold_shape(100, 1000, 100, 10_000, 0.9, 0.0);
        assert_eq!(choice, ColdIndexChoice::MmapOrdered);
    }

    #[test]
    fn test_budget_refuses_over_limit_write() {
        let budget = PkIndexBudget { max_bytes: 100 };
        assert!(budget.check(80).is_ok());
        let err = budget.check(120).unwrap_err();
        assert!(err.to_string().contains("exceeds budget"));
        assert!((budget.pressure_ratio(50) - 0.5).abs() < f64::EPSILON);
    }
}
