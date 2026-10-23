//! Unified background maintenance scheduling.
//!
//! Several storage subsystems run recurring background work with independent
//! triggers: CSR maintenance (mutable-CSR pressure), index GC (tombstone
//! pressure after writes), vertex GC (periodic tombstone scan), checkpoint
//! polling, TTL purge sweeps, and WAL flushes. Merging their logic into one
//! task would couple unrelated triggers, so this module unifies only the
//! scheduling layer: single-flight, completion-time cooldown, exponential
//! no-progress backoff, panic-safe execution, timeout observation, and
//! shared diagnostics.
//!
//! Tasks that fit the run-to-completion model own a [`MaintenanceTask`]
//! (CSR maintenance, opportunistic index GC, index/vertex GC passes, TTL
//! sweeps). Polling loops with their own idle semantics (checkpoint) keep
//! their triggers and reuse [`execute_guarded`] for panic-safe,
//! timeout-observed execution.
//!
//! Scheduling vocabulary used across every task:
//!
//! - `interval` (owned by [`crate::thread_pool::StorageThreadPool::spawn_periodic`])
//!   is the maximum wake-up cadence: a loop wakes at most that often.
//! - `min_cooldown` (owned by [`MaintenanceTask`]) is the minimum gap between
//!   two completed passes, measured from the previous completion and
//!   multiplied by the no-progress backoff. Throttled passes are skipped,
//!   not queued.
//!
//! Timeouts are warn-only and never cancel a pass: maintenance passes hold
//! table locks and must run to a safe point, so overruns are reported for
//! tuning while the pass completes. Checkpoint execution additionally runs
//! on a nested pool task so a slow checkpoint never blocks its poll loop.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::thread_pool::StorageThreadPool;

/// Per-task scheduling policy.
#[derive(Debug, Clone)]
pub struct MaintenanceTaskConfig {
    /// Stable task name used in logs and diagnostics.
    pub name: &'static str,
    /// Minimum gap between two completed runs. Zero disables throttling.
    pub min_cooldown: Duration,
    /// Backoff multiplier is `1 << min(backoff, max_backoff_shift)`.
    pub max_backoff_shift: u32,
    /// Runs exceeding this duration are reported but never cancelled.
    pub timeout: Duration,
}

impl MaintenanceTaskConfig {
    pub fn new(name: &'static str, min_cooldown: Duration, timeout: Duration) -> Self {
        Self {
            name,
            min_cooldown,
            max_backoff_shift: 6,
            timeout,
        }
    }
}

/// Point-in-time scheduling state for one maintenance task.
#[derive(Debug, Clone)]
pub struct MaintenanceDiagnostics {
    pub name: &'static str,
    pub running: bool,
    pub backoff: u32,
    pub total_runs: u64,
    pub progress_runs: u64,
    pub last_progress: bool,
    pub last_duration_ms: Option<u128>,
    pub millis_since_completion: Option<u128>,
}

impl MaintenanceDiagnostics {
    /// Single-line human-readable state for logs.
    pub fn summary(&self) -> String {
        format!(
            "{}: running={} backoff={} total_runs={} progress_runs={} last_progress={} last_duration_ms={:?} millis_since_completion={:?}",
            self.name,
            self.running,
            self.backoff,
            self.total_runs,
            self.progress_runs,
            self.last_progress,
            self.last_duration_ms,
            self.millis_since_completion,
        )
    }
}

/// One observability snapshot across every independent background task.
/// Tasks keep their own triggers and intervals; only lifecycle and
/// scheduling state are reported together.
///
/// Field semantics: each entry in `tasks` reports a single pass
/// (`running` means one pass is in flight right now), while the
/// `*_loop_running` / `*_running` flags report whether the periodic loop
/// itself is scheduled. A scheduled loop with no pass in flight is the
/// normal idle state, not a stall.
#[derive(Debug, Clone)]
pub struct MaintenanceOverview {
    /// Unified per-task scheduling states (CSR maintenance, index GC,
    /// vertex GC, TTL purge when configured). `running` here is pass in-flight.
    pub tasks: Vec<MaintenanceDiagnostics>,
    /// Checkpoint poll loop scheduled.
    pub checkpoint_running: bool,
    /// Checkpoint requested but not yet executed.
    pub checkpoint_pending: bool,
    /// Checkpoint executions currently in flight on the pool.
    pub checkpoint_active: usize,
    /// TTL purge sweep scheduled.
    pub ttl_purge_running: bool,
    /// Index GC background loop scheduled (not pass in-flight; see `tasks`).
    pub index_gc_loop_running: bool,
    /// Vertex GC background loop scheduled (not pass in-flight; see `tasks`).
    pub vertex_gc_loop_running: bool,
    /// Index GC loop started automatically on pressure (vs explicit start).
    pub index_gc_auto_started: bool,
    /// Vertex GC loop started automatically on pressure (vs explicit start).
    pub vertex_gc_auto_started: bool,
    /// Idle CSR-maintenance fallback driver scheduled.
    pub idle_maintenance_running: bool,
}

impl MaintenanceOverview {
    /// Single-line human-readable state for logs.
    pub fn summary(&self) -> String {
        let tasks: Vec<String> = self.tasks.iter().map(|task| task.summary()).collect();
        format!(
            "tasks=[{}] checkpoint_running={} checkpoint_pending={} checkpoint_active={} ttl_purge_running={} index_gc_loop_running={} vertex_gc_loop_running={} index_gc_auto_started={} vertex_gc_auto_started={} idle_maintenance_running={}",
            tasks.join(", "),
            self.checkpoint_running,
            self.checkpoint_pending,
            self.checkpoint_active,
            self.ttl_purge_running,
            self.index_gc_loop_running,
            self.vertex_gc_loop_running,
            self.index_gc_auto_started,
            self.vertex_gc_auto_started,
            self.idle_maintenance_running,
        )
    }
}

/// Shared scheduling state for one background maintenance task.
///
/// Clones share every counter so a task spawned on a clone still reports
/// through the originating handle.
pub struct MaintenanceTask {
    name: &'static str,
    min_cooldown: Mutex<Duration>,
    max_backoff_shift: u32,
    timeout: Mutex<Duration>,
    pool: Arc<StorageThreadPool>,
    running: Arc<AtomicBool>,
    last_completion: Arc<Mutex<Option<Instant>>>,
    backoff: Arc<AtomicU32>,
    total_runs: Arc<AtomicU64>,
    progress_runs: Arc<AtomicU64>,
    last_duration: Arc<Mutex<Option<Duration>>>,
    last_progress: Arc<AtomicBool>,
}

impl MaintenanceTask {
    pub fn new(pool: Arc<StorageThreadPool>, config: MaintenanceTaskConfig) -> Self {
        Self {
            name: config.name,
            min_cooldown: Mutex::new(config.min_cooldown),
            max_backoff_shift: config.max_backoff_shift,
            timeout: Mutex::new(config.timeout),
            pool,
            running: Arc::new(AtomicBool::new(false)),
            last_completion: Arc::new(Mutex::new(None)),
            backoff: Arc::new(AtomicU32::new(0)),
            total_runs: Arc::new(AtomicU64::new(0)),
            progress_runs: Arc::new(AtomicU64::new(0)),
            last_duration: Arc::new(Mutex::new(None)),
            last_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_min_cooldown(&self, cooldown: Duration) {
        *self.min_cooldown.lock() = cooldown;
    }

    pub fn set_timeout(&self, timeout: Duration) {
        *self.timeout.lock() = timeout;
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Block until no pass is in flight, up to `timeout`. Returns whether
    /// the task reached idle in time. Never forces a running pass to stop;
    /// callers use the result to decide whether shutdown may proceed.
    pub fn wait_until_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.is_running() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }

    pub fn backoff(&self) -> u32 {
        self.backoff.load(Ordering::Acquire)
    }

    pub fn total_runs(&self) -> u64 {
        self.total_runs.load(Ordering::Acquire)
    }

    pub fn progress_runs(&self) -> u64 {
        self.progress_runs.load(Ordering::Acquire)
    }

    fn effective_cooldown(&self) -> Duration {
        effective_cooldown(
            *self.min_cooldown.lock(),
            self.backoff.load(Ordering::Relaxed),
            self.max_backoff_shift,
        )
    }

    fn cooldown_active(&self) -> bool {
        let cooldown = self.effective_cooldown();
        if cooldown.is_zero() {
            return false;
        }
        let guard = self.last_completion.lock();
        guard.is_some_and(|prev| prev.elapsed() < cooldown)
    }

    fn claim(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn release(&self) {
        self.running.store(false, Ordering::Release);
    }

    /// Acquire the single-flight permit and pass the completion-cooldown
    /// gate. Returns `true` with `running` held when the caller may proceed;
    /// returns `false` (permit released or never taken) when another pass is
    /// in flight or the cooldown with backoff has not elapsed.
    fn acquire_permit(&self) -> bool {
        if !self.claim() {
            return false;
        }
        if self.cooldown_active() {
            self.release();
            return false;
        }
        true
    }

    /// Shared completion bookkeeping for [`Self::tick`]: backoff reset on
    /// progress (capped increment otherwise), run counters, completion
    /// timestamp, and permit release. Timeout overruns are reported once by
    /// [`execute_guarded`]; this path stays silent so a slow pass warns
    /// exactly once.
    fn record_result(&self, progressed: bool, elapsed: Duration) {
        record_completion(
            progressed,
            elapsed,
            &self.backoff,
            &self.total_runs,
            &self.progress_runs,
            &self.last_progress,
            &self.last_duration,
            &self.last_completion,
            self.max_backoff_shift,
        );
        self.release();
    }

    /// Schedule `task` on the shared pool. Returns `false` without spawning
    /// when another run is in flight or the completion cooldown (with
    /// backoff) has not elapsed. The boolean returned by `task` reports
    /// whether the run made progress and drives backoff reset.
    pub fn try_spawn<F>(&self, task: F) -> bool
    where
        F: FnOnce() -> bool + Send + 'static,
    {
        self.try_spawn_with_timeout(task, *self.timeout.lock())
    }

    /// Same as [`try_spawn`](Self::try_spawn) with a per-spawn timeout used
    /// only for overrun reporting.
    fn try_spawn_with_timeout<F>(&self, task: F, timeout: Duration) -> bool
    where
        F: FnOnce() -> bool + Send + 'static,
    {
        if !self.acquire_permit() {
            return false;
        }
        // Shared completion path with `tick`: the spawned closure owns
        // clones of every counter so the originating handle stays current.
        // `RunningGuard` is belt-and-braces for paths that return early
        // before the shared completion below; the normal path releases the
        // permit there.
        let running = self.running.clone();
        let completion = self.last_completion.clone();
        let backoff = self.backoff.clone();
        let total_runs = self.total_runs.clone();
        let progress_runs = self.progress_runs.clone();
        let last_duration = self.last_duration.clone();
        let last_progress = self.last_progress.clone();
        let name = self.name;
        let max_shift = self.max_backoff_shift;
        self.pool.spawn(move || {
            let _guard = RunningGuard { running };
            let started = Instant::now();
            let progressed = execute_guarded(name, timeout, task);
            let elapsed = started.elapsed();
            apply_spawned_result(
                progressed,
                elapsed,
                &backoff,
                &total_runs,
                &progress_runs,
                &last_progress,
                &last_duration,
                &completion,
                max_shift,
            );
        });
        true
    }

    /// Run `task` inline with the same throttle, backoff, panic-safety, and
    /// timeout semantics. Used by periodic background loops: returns `None`
    /// when the pass is throttled, otherwise the progress flag.
    pub fn tick<F>(&self, task: F) -> Option<bool>
    where
        F: FnOnce() -> bool,
    {
        self.tick_with_timeout(task, *self.timeout.lock())
    }

    /// Same as [`tick`](Self::tick) with a per-pass timeout override.
    fn tick_with_timeout<F>(&self, task: F, timeout: Duration) -> Option<bool>
    where
        F: FnOnce() -> bool,
    {
        if !self.acquire_permit() {
            return None;
        }
        let _guard = RunningGuard {
            running: self.running.clone(),
        };
        let started = Instant::now();
        let progressed = execute_guarded(self.name, timeout, task);
        self.record_result(progressed, started.elapsed());
        Some(progressed)
    }

    pub fn diagnostics(&self) -> MaintenanceDiagnostics {
        let (last_duration_ms, millis_since_completion) = {
            let duration = self.last_duration.lock();
            let completion = self.last_completion.lock();
            (
                duration.as_ref().map(Duration::as_millis),
                completion
                    .as_ref()
                    .map(Instant::elapsed)
                    .map(|d| d.as_millis()),
            )
        };
        MaintenanceDiagnostics {
            name: self.name,
            running: self.is_running(),
            backoff: self.backoff(),
            total_runs: self.total_runs(),
            progress_runs: self.progress_runs(),
            last_progress: self.last_progress.load(Ordering::Acquire),
            last_duration_ms,
            millis_since_completion,
        }
    }
}

/// Completion-gap multiplier shared by every maintenance task: the base
/// cooldown grows as `base * 2^backoff` (shift capped) so consecutive
/// no-progress runs back off exponentially, and any progressing run resets
/// the counter to zero.
pub(crate) fn effective_cooldown(base: Duration, backoff: u32, max_shift: u32) -> Duration {
    if base.is_zero() {
        return Duration::ZERO;
    }
    base.saturating_mul(1u32 << backoff.min(max_shift))
}

/// Shared completion bookkeeping behind both [`MaintenanceTask::tick`] and
/// the [`MaintenanceTask::try_spawn`] closure: backoff reset on progress
/// (capped increment otherwise), run counters, and completion timestamp.
/// Deliberately silent on timeout overruns: [`execute_guarded`] already
/// reported them, so warning here would log every slow pass twice.
#[allow(clippy::too_many_arguments)]
fn record_completion(
    progressed: bool,
    elapsed: Duration,
    backoff: &Arc<AtomicU32>,
    total_runs: &Arc<AtomicU64>,
    progress_runs: &Arc<AtomicU64>,
    last_progress: &Arc<AtomicBool>,
    last_duration: &Arc<Mutex<Option<Duration>>>,
    last_completion: &Arc<Mutex<Option<Instant>>>,
    max_backoff_shift: u32,
) {
    if progressed {
        backoff.store(0, Ordering::Relaxed);
        progress_runs.fetch_add(1, Ordering::Release);
    } else {
        let current = backoff.load(Ordering::Relaxed);
        backoff.store(
            current.saturating_add(1).min(max_backoff_shift),
            Ordering::Relaxed,
        );
    }
    total_runs.fetch_add(1, Ordering::Release);
    last_progress.store(progressed, Ordering::Release);
    *last_duration.lock() = Some(elapsed);
    *last_completion.lock() = Some(Instant::now());
}

/// Completion path for a spawned pool closure, which cannot borrow the
/// originating [`MaintenanceTask`] and works on clones instead. Same
/// counters as [`record_completion`]; the caller holds a `RunningGuard` so
/// the permit clears even if this is never reached.
#[allow(clippy::too_many_arguments)]
fn apply_spawned_result(
    progressed: bool,
    elapsed: Duration,
    backoff: &Arc<AtomicU32>,
    total_runs: &Arc<AtomicU64>,
    progress_runs: &Arc<AtomicU64>,
    last_progress: &Arc<AtomicBool>,
    last_duration: &Arc<Mutex<Option<Duration>>>,
    last_completion: &Arc<Mutex<Option<Instant>>>,
    max_backoff_shift: u32,
) {
    record_completion(
        progressed,
        elapsed,
        backoff,
        total_runs,
        progress_runs,
        last_progress,
        last_duration,
        last_completion,
        max_backoff_shift,
    );
}

/// Clears the running flag on drop so a panicking task cannot permanently
/// disable its scheduler.
struct RunningGuard {
    running: Arc<AtomicBool>,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

/// Runs `task` to completion, converting panics into logged no-progress
/// results and warning on timeout overruns. Never propagates a panic.
pub(crate) fn execute_guarded<F>(name: &'static str, timeout: Duration, task: F) -> bool
where
    F: FnOnce() -> bool,
{
    let started = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
    let elapsed = started.elapsed();
    match result {
        Ok(progressed) => {
            if elapsed > timeout {
                log::warn!(
                    "Maintenance task '{}' exceeded timeout: {:?} > {:?}",
                    name,
                    elapsed,
                    timeout
                );
            }
            progressed
        }
        Err(_) => {
            log::warn!(
                "Maintenance task '{}' panicked; counted as no progress",
                name
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pool() -> Arc<StorageThreadPool> {
        Arc::new(StorageThreadPool::new().unwrap())
    }

    fn test_task(cooldown: Duration) -> MaintenanceTask {
        MaintenanceTask::new(
            test_pool(),
            MaintenanceTaskConfig::new("test-task", cooldown, Duration::from_secs(30)),
        )
    }

    #[test]
    fn effective_cooldown_doubles_with_backoff_and_caps_shift() {
        let base = Duration::from_millis(100);
        assert_eq!(effective_cooldown(base, 0, 6), base);
        assert_eq!(effective_cooldown(base, 1, 6), Duration::from_millis(200));
        assert_eq!(effective_cooldown(base, 6, 6), Duration::from_millis(6400));
        assert_eq!(effective_cooldown(base, 10, 6), Duration::from_millis(6400));
        assert_eq!(effective_cooldown(Duration::ZERO, 5, 6), Duration::ZERO);
    }

    #[test]
    fn tick_records_completion_and_backoff() {
        let task = test_task(Duration::ZERO);
        assert_eq!(task.tick(|| true), Some(true));
        assert_eq!(task.backoff(), 0);
        assert_eq!(task.total_runs(), 1);
        assert_eq!(task.progress_runs(), 1);
        assert_eq!(task.tick(|| false), Some(false));
        assert_eq!(task.backoff(), 1);
        assert_eq!(task.total_runs(), 2);
        assert_eq!(task.tick(|| true), Some(true));
        assert_eq!(task.backoff(), 0);
    }

    #[test]
    fn tick_throttles_within_completion_cooldown() {
        let task = test_task(Duration::from_secs(60));
        assert_eq!(task.tick(|| true), Some(true));
        assert_eq!(task.tick(|| true), None);
        assert_eq!(task.total_runs(), 1);
    }

    #[test]
    fn tick_backoff_extends_cooldown() {
        // Base 100ms: one no-progress pass doubles the gate to 200ms. A
        // 150ms wait clears the base gap but must still be throttled (proves
        // the backoff extension); after 250ms total the pass runs and resets.
        let task = test_task(Duration::from_millis(100));
        assert_eq!(task.tick(|| false), Some(false));
        assert_eq!(task.backoff(), 1);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(task.tick(|| true), None);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(task.tick(|| true), Some(true));
        assert_eq!(task.backoff(), 0);
    }

    #[test]
    fn tick_is_panic_safe_and_counts_no_progress() {
        let task = test_task(Duration::ZERO);
        let result = task.tick(|| panic!("injected"));
        assert_eq!(result, Some(false));
        assert!(!task.is_running());
        assert_eq!(task.backoff(), 1);
        assert_eq!(task.tick(|| true), Some(true));
    }

    #[test]
    fn try_spawn_shares_cooldown_semantics_with_tick() {
        // `try_spawn` and `tick` share one permit gate: a completed spawn
        // arms the same completion cooldown a tick would observe.
        let task = test_task(Duration::from_secs(60));
        assert!(task.try_spawn(|| true));
        for _ in 0..100 {
            if task.total_runs() >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(task.total_runs(), 1);
        assert!(!task.try_spawn(|| true));
        assert_eq!(task.tick(|| true), None);
        assert_eq!(task.total_runs(), 1);
    }

    #[test]
    fn try_spawn_is_single_flight_and_panic_safe() {
        let task = test_task(Duration::ZERO);
        assert!(task.try_spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            true
        }));
        assert!(!task.try_spawn(|| true));
        for _ in 0..100 {
            if !task.is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!task.is_running());
        assert_eq!(task.total_runs(), 1);
        assert!(task.try_spawn(|| panic!("injected")));
        for _ in 0..100 {
            if task.total_runs() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(task.total_runs(), 2);
        assert!(!task.is_running());
    }

    #[test]
    fn diagnostics_reports_state() {
        let task = test_task(Duration::ZERO);
        task.tick(|| true);
        let diagnostics = task.diagnostics();
        assert_eq!(diagnostics.name, "test-task");
        assert!(!diagnostics.running);
        assert_eq!(diagnostics.backoff, 0);
        assert_eq!(diagnostics.total_runs, 1);
        assert_eq!(diagnostics.progress_runs, 1);
        assert!(diagnostics.last_progress);
        assert!(diagnostics.last_duration_ms.is_some());
        assert!(diagnostics.millis_since_completion.is_some());
    }

    #[test]
    fn wait_until_idle_returns_immediately_when_idle() {
        let task = test_task(Duration::ZERO);
        assert!(task.wait_until_idle(Duration::from_millis(10)));
    }

    #[test]
    fn wait_until_idle_times_out_while_pass_in_flight() {
        let task = test_task(Duration::ZERO);
        assert!(task.try_spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            true
        }));
        assert!(!task.wait_until_idle(Duration::from_millis(10)));
        assert!(task.wait_until_idle(Duration::from_secs(5)));
    }
}
