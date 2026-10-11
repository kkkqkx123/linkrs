// Criterion group setup shared by the benchmark binaries.
//
// Single source of truth for the benchmark run convention. The query and
// storage crates reuse this file through a one-line include shim so the
// windows cannot drift between copies.
//
// Run convention: default is the full run with long measurement windows for
// stable numbers; setting `QUICK=1` switches to a fixed fast run with short
// windows and small samples for smoke runs. A full run takes minutes per
// suite, a fast run finishes in seconds.
//
// Plain-main benches (harness = false with `fn main`) read the same flag
// through `quick_mode` to shrink their own workloads.

use criterion::Criterion;
use std::time::Duration;

/// Whether the run requests the fast smoke run (`QUICK` set in the environment).
pub fn quick_mode() -> bool {
    std::env::var("QUICK").is_ok()
}

/// Effective sample size for the requested full-run size.
pub fn effective_sample_size(requested: usize) -> usize {
    if quick_mode() {
        requested.min(10).max(2)
    } else {
        requested
    }
}

/// Open a criterion group with the shared warm-up and measurement windows.
///
/// `sample_size` is the only per-suite knob: a suite that wants more
/// statistical confidence raises it, and one that keeps each sample expensive
/// lowers it. The windows stay identical so numbers from different suites
/// remain comparable.
pub fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
    sample_size: usize,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    if quick_mode() {
        group.measurement_time(Duration::from_millis(800));
        group.sample_size(effective_sample_size(sample_size));
        group.warm_up_time(Duration::from_millis(200));
    } else {
        group.measurement_time(Duration::from_secs(10));
        group.sample_size(sample_size);
        group.warm_up_time(Duration::from_secs(1));
    }
    group
}
