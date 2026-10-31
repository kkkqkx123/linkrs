//! Benchmark references for registry coverage gates.

/// Benchmark sources pinning the linked bench functions.
#[derive(Debug, Clone, Copy)]
pub(crate) enum BenchFile {
    StorageBench,
    CsrPerfBench,
    CommitGateBench,
    WalBench,
    CrashRecoveryBench,
    StorageWorkflowBench,
    EdgeReadAllocBench,
}

const ALL_BENCH_FILES: &[BenchFile] = &[
    BenchFile::StorageBench,
    BenchFile::CsrPerfBench,
    BenchFile::CommitGateBench,
    BenchFile::WalBench,
    BenchFile::CrashRecoveryBench,
    BenchFile::StorageWorkflowBench,
    BenchFile::EdgeReadAllocBench,
];

impl BenchFile {
    pub(crate) fn all() -> &'static [BenchFile] {
        ALL_BENCH_FILES
    }

    pub(crate) fn source(self) -> &'static str {
        match self {
            BenchFile::StorageBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/storage_bench.rs"
            )),
            BenchFile::CsrPerfBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/csr_perf_bench.rs"
            )),
            BenchFile::CommitGateBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/commit_gate_bench.rs"
            )),
            BenchFile::WalBench => {
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/benches/wal_bench.rs"))
            }
            BenchFile::CrashRecoveryBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/crash_recovery_bench.rs"
            )),
            BenchFile::StorageWorkflowBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/storage_workflow_bench.rs"
            )),
            BenchFile::EdgeReadAllocBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/edge_read_alloc_bench.rs"
            )),
        }
    }

    /// Criterion benches register in `criterion_group!`; plain-main benches
    /// must be called from `fn main(`.
    pub(crate) fn is_criterion(self) -> bool {
        !matches!(self, BenchFile::CsrPerfBench | BenchFile::CommitGateBench)
    }

    /// Region of the source that must mention a bench function for the
    /// function to actually run.
    pub(crate) fn registration_block(self) -> &'static str {
        let source = self.source();
        if self.is_criterion() {
            let start = source
                .find("criterion_group!(")
                .expect("criterion bench file holds a criterion group");
            let end = source
                .find("criterion_main!")
                .expect("criterion bench file holds a criterion main");
            &source[start..end]
        } else {
            let start = source
                .find("fn main(")
                .expect("plain bench file holds a main function");
            &source[start..]
        }
    }
}

pub(crate) fn find_bench_file(bench_fn: &str) -> Option<BenchFile> {
    let definition = format!("fn {bench_fn}(");
    BenchFile::all()
        .iter()
        .copied()
        .find(|file| file.source().contains(&definition))
}
