//! VertexTable module organization
//!
//! Split into logical components:
//! - core: CRUD operations and queries
//! - persistence: File I/O, serialization
//! - optimizer: Compaction and optimization
//! - schema: Schema management
//! - compaction: Unified compaction coordinator (medium-term improvement)

pub mod compaction;
pub mod core;
pub mod flush_trigger;
pub mod optimizer;
pub mod persistence;
pub mod schema;
pub mod sharded;
pub mod staged_schema;

pub(crate) use sharded::persistence::CommitKind;
pub use sharded::ShardedVertexTable;

#[cfg(test)]
mod bench_coverage_tests {
    //! R6 gate: every tunable storage threshold is either exercised by a
    //! registered benchmark group or explicitly exempted with a reason.
    //! Retuning a threshold without bench coverage fails this test by
    //! construction: the registry below pins each constant (renames break
    //! the build) and each bench function (removal breaks the assertion).

    const BENCH_SOURCE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../benches/storage_bench.rs"
    ));

    /// One policy knob: the constant it tunes and the bench function
    /// covering it, or `None` with an exemption reason when no bench can
    /// observe the knob (correctness markers, unwired policy).
    struct CoverageEntry {
        threshold: &'static str,
        bench_fn: Option<&'static str>,
        reason: &'static str,
    }

    const REGISTRY: &[CoverageEntry] = &[
        CoverageEntry {
            threshold: "PK_DELTA_ANCHOR_THRESHOLD",
            bench_fn: Some("bench_vertex_churn_reuse"),
            reason: "churn drives delta growth past the anchor threshold",
        },
        CoverageEntry {
            threshold: "anchor_threshold_for_live",
            bench_fn: Some("bench_vertex_churn_reuse"),
            reason: "scale-aware anchor under delete plus reinsert load",
        },
        CoverageEntry {
            threshold: "STUCK_SNAPSHOT_AGE_SECS",
            bench_fn: Some("bench_vertex_churn_reuse"),
            reason: "churn plus version pressure is the stuck-watermark load",
        },
        CoverageEntry {
            threshold: "MAX_BASELINE_AGE_MS",
            bench_fn: Some("bench_scaled_checkpoint"),
            reason: "scaled checkpoint measures baseline rewrite cost",
        },
        CoverageEntry {
            threshold: "PAGE_MERGE_RATIO",
            bench_fn: Some("bench_real_checkpoint"),
            reason: "real checkpoint covers page consolidation behavior",
        },
        CoverageEntry {
            threshold: "SHARD_FRAGMENTATION_THRESHOLD",
            bench_fn: Some("bench_scan_all_vertices"),
            reason: "fragmentation shows up as scan degradation",
        },
        CoverageEntry {
            threshold: "EVICTION_SEGMENT_BYTES",
            bench_fn: Some("bench_scaled_cursor_scan"),
            reason: "scaled scans drive residency and eviction",
        },
        CoverageEntry {
            threshold: "max_lease_ttl_ms",
            bench_fn: Some("bench_vertex_churn_reuse"),
            reason: "lease pressure under churn plus version load; issuance and renewal are transaction-driven with injected-clock tests",
        },
        CoverageEntry {
            threshold: "default_lease_ttl/long_read_max_live_leases/long_read_max_pin_secs",
            bench_fn: Some("bench_vertex_churn_reuse"),
            reason: "transaction-side lease terms and admission thresholds under the stuck-watermark load",
        },
        CoverageEntry {
            threshold: "TIERING_MIN_PROBES",
            bench_fn: Some("bench_vertex_point_lookup"),
            reason: "gate evidence floor rides point-lookup probe traffic",
        },
        CoverageEntry {
            threshold: "TIERING_SKEW_THRESHOLD",
            bench_fn: Some("bench_vertex_point_lookup"),
            reason: "concentration signal read from point-lookup probe skew",
        },
        CoverageEntry {
            threshold: "TIERING_HOT_SHARE_THRESHOLD",
            bench_fn: Some("bench_vertex_point_lookup"),
            reason: "hot-share cutoff scored against point-lookup probe distribution",
        },
        CoverageEntry {
            threshold: "pk_index_budget_bytes",
            bench_fn: None,
            reason: "unset (0) by default with zero serving-path effect; over-limit refusal is covered by tiering unit tests",
        },
        CoverageEntry {
            threshold: "DEFAULT_DRAIN_TIMEOUT_MS",
            bench_fn: None,
            reason: "online-migration drain bound; overrun rollback is covered by migration session unit tests with no steady-state bench signal",
        },
        CoverageEntry {
            threshold: "ROUTER_VERSION/generation",
            bench_fn: None,
            reason: "correctness markers, not tunable thresholds; covered by fault-injection tests instead of benches",
        },
    ];

    /// Gate subsets: a threshold-group change runs its subset, not the
    /// full bench suite. Each entry pins subset bench functions; the test
    /// below fails when a subset names an unregistered bench.
    const SUBSETS: &[(&str, &[&str])] = &[
        ("delta_anchor", &["bench_vertex_churn_reuse"]),
        ("snapshot_watermark", &["bench_vertex_churn_reuse"]),
        (
            "baseline_rewrite",
            &["bench_scaled_checkpoint", "bench_real_checkpoint"],
        ),
        ("page_merge", &["bench_real_checkpoint"]),
        (
            "scan_fragmentation",
            &["bench_scan_all_vertices", "bench_scaled_cursor_scan"],
        ),
        ("residency_eviction", &["bench_scaled_cursor_scan"]),
        ("point_lookup", &["bench_vertex_point_lookup"]),
        ("tiering_gate", &["bench_vertex_point_lookup"]),
        ("lease_policy", &["bench_vertex_churn_reuse"]),
    ];

    fn group_block() -> &'static str {
        let start = BENCH_SOURCE
            .find("criterion_group!(")
            .expect("bench file holds a criterion group");
        let end = BENCH_SOURCE
            .find("criterion_main!")
            .expect("bench file holds a criterion main");
        &BENCH_SOURCE[start..end]
    }

    #[test]
    fn test_policy_thresholds_are_bench_covered_or_exempt() {
        // Compile-time existence pins: renaming a constant breaks this.
        let _ = crate::vertex::id_indexer::PK_DELTA_ANCHOR_THRESHOLD;
        let _ = crate::vertex::id_indexer::IdManager::anchor_threshold_for_live;
        let _ = crate::vertex::gc_manager::STUCK_SNAPSHOT_AGE_SECS;
        let _ = super::flush_trigger::MAX_BASELINE_AGE_MS;
        let _ = super::flush_trigger::MIN_BASELINE_AGE_MS;
        let _ = super::flush_trigger::PAGE_MERGE_RATIO;
        let _ = super::sharded::maintenance::SHARD_FRAGMENTATION_THRESHOLD;
        let _ = crate::vertex::column::EVICTION_SEGMENT_BYTES;
        let _ = crate::vertex::column::MAX_BACKGROUND_LOAD_CHUNKS;
        let _ = crate::vertex::gc_manager::VertexGcConfig::default().max_lease_ttl_ms;
        let _ = crate::vertex::tiering::TIERING_MIN_PROBES;
        let _ = crate::vertex::tiering::TIERING_SKEW_THRESHOLD;
        let _ = crate::vertex::tiering::TIERING_HOT_SHARE_THRESHOLD;
        let _ = crate::vertex::vertex_table::sharded::migration::DEFAULT_DRAIN_TIMEOUT_MS;
        let _ = crate::engine::config::PropertyGraphConfig::default().pk_index_budget_bytes;
        let _ = graphdb_transaction::TransactionManagerConfig::default().default_lease_ttl;
        let _ = graphdb_transaction::TransactionManagerConfig::default().long_read_max_live_leases;
        let _ = graphdb_transaction::TransactionManagerConfig::default().long_read_max_pin_secs;

        let group = group_block();
        for entry in REGISTRY {
            match entry.bench_fn {
                Some(bench_fn) => {
                    assert!(
                        BENCH_SOURCE.contains(&format!("fn {bench_fn}(")),
                        "threshold {} lost its bench function {bench_fn}",
                        entry.threshold,
                    );
                    assert!(
                        group.contains(bench_fn),
                        "threshold {} bench {bench_fn} is not registered in criterion_group!",
                        entry.threshold,
                    );
                    assert!(
                        !entry.reason.is_empty(),
                        "coverage claim for {} needs a reason",
                        entry.threshold,
                    );
                }
                None => assert!(
                    !entry.reason.is_empty(),
                    "exemption for {} needs a reason",
                    entry.threshold,
                ),
            }
        }
    }

    #[test]
    fn test_every_bench_fn_is_registered() {
        let group = group_block();
        for line in BENCH_SOURCE.lines() {
            let name = line.trim().strip_prefix("fn ").unwrap_or("");
            let name = name.split('(').next().unwrap_or("").trim();
            if name.starts_with("bench_") {
                assert!(
                    group.contains(name),
                    "bench function {name} is defined but not registered in criterion_group!",
                );
            }
        }
    }

    #[test]
    fn test_gate_subsets_reference_registered_benches() {
        let group = group_block();
        let mut names: Vec<&str> = SUBSETS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            SUBSETS.len(),
            "gate subset names must be unique: {names:?}"
        );
        for (subset, benches) in SUBSETS {
            assert!(
                !benches.is_empty(),
                "gate subset {subset} must name benches"
            );
            for bench_fn in *benches {
                assert!(
                    BENCH_SOURCE.contains(&format!("fn {bench_fn}(")),
                    "gate subset {subset} references missing bench function {bench_fn}",
                );
                assert!(
                    group.contains(bench_fn),
                    "gate subset {subset} bench {bench_fn} is not registered in criterion_group!",
                );
            }
        }
    }
}
