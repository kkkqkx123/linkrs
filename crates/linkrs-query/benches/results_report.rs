//! Result-report writing shared by the benchmark binaries.
//!
//! Included per target with `#[path = "results_report.rs"] mod report;`
//! because a benchmark crate root cannot be shared otherwise: cargo compiles
//! each `[[bench]]` as its own crate, so a helper has to be pulled in by
//! path.

/// Write a human-readable report into `benches/results/<bench_name>/`.
/// Returns the file path written.
pub fn write_results_report(bench_name: &str, filename: &str, content: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("benches")
        .join("results")
        .join(bench_name);
    std::fs::create_dir_all(&dir).expect("create results dir");
    let path = dir.join(filename);
    std::fs::write(&path, content).expect("write results report");
    path
}
