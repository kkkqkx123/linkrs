//! Unified background execution surface.
//!
//! One-shot maintenance passes run on a shared rayon pool, while periodic
//! loops run on dedicated driver threads. Drivers only wait and dispatch;
//! heavy parallel work inside a pass still fans out through the shared pool,
//! so sleeping loops never occupy rayon workers needed by GC, freeze, and
//! checkpoint passes. All drivers share the cooperative stop/join contract
//! of [`BackgroundTaskHandle`].
//!
//! One deliberate exception is the WAL async-flush thread
//! (`WalFlushCoordinator` in `linkrs-transaction`): it stays a dedicated I/O
//! thread outside this surface so blocking file writes and condvar waits can
//! never occupy rayon workers shared with GC, freeze, and checkpoint passes.
//! Its lifecycle is still joinable (`shutdown()` blocks until the thread
//! exits, and `Drop` shuts it down), matching the stop/join semantics of
//! the periodic loops below. WAL flush health is observed through WAL
//! metrics, not [`crate::maintenance_scheduler::MaintenanceOverview`].
//!
//! Timeouts across every task are warn-only and never cancel a pass:
//! maintenance passes hold table locks and must run to a safe point.
//!
//! Two flavors of work are supported:
//!
//! - [`StorageThreadPool::spawn`] — fire-and-forget one-shot tasks (e.g. the
//!   background freeze/maintenance pass) on the shared pool.
//! - [`StorageThreadPool::spawn_periodic`] — loops on a dedicated driver
//!   thread that run until their [`BackgroundTaskHandle::stop`] is called,
//!   with a handle that supports blocking [`BackgroundTaskHandle::join`]
//!   for graceful shutdown.

use linkrs_core::{StorageError, StorageResult};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Handle to a periodic background task driven by a dedicated thread.
///
/// The handle shares the task's `running` flag, so [`stop`](Self::stop) is
/// observed by the loop within one interval plus the current pass duration.
/// [`join`](Self::join) blocks until the loop has fully exited.
#[derive(Debug, Clone)]
pub struct BackgroundTaskHandle {
    running: Arc<AtomicBool>,
    finished: Arc<Mutex<bool>>,
    condvar: Arc<Condvar>,
}

impl BackgroundTaskHandle {
    /// Request the periodic loop to stop at its next safe point.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    /// Whether the task is still scheduled to run.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Block until the task loop exits (after `stop()`).
    pub fn join(&self) {
        let mut finished = self.finished.lock();
        while !*finished {
            self.condvar.wait(&mut finished);
        }
    }
}

/// Shared background execution surface.
///
/// Clones share the same underlying rayon worker threads for one-shot work.
/// The pool is sized from `available_parallelism` (clamped) so background
/// maintenance never spawns unbounded OS threads. Periodic loops additionally
/// consume one dedicated driver thread each; drivers only wait between
/// passes and never occupy pool workers.
#[derive(Clone)]
pub struct StorageThreadPool {
    inner: Arc<rayon::ThreadPool>,
}

impl std::fmt::Debug for StorageThreadPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageThreadPool")
            .field("num_threads", &self.inner.current_num_threads())
            .finish()
    }
}

impl StorageThreadPool {
    /// Build a pool sized from the available parallelism, clamped to a sane
    /// range so background maintenance never spawns unbounded OS threads.
    pub fn new() -> StorageResult<Self> {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(2, 16);
        Self::with_threads(threads)
    }

    /// Build a pool with an explicit thread count (clamped to `[1, 16]`).
    /// Used for dedicated single-thread execution channels that must stay
    /// decoupled from the shared compute pool.
    pub fn with_threads(threads: usize) -> StorageResult<Self> {
        let threads = threads.clamp(1, 16);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|idx| format!("linkrs-storage-{idx}"))
            .build()
            .map_err(|e| {
                StorageError::storage_error(format!("failed to build thread pool: {e}"))
            })?;
        Ok(Self {
            inner: Arc::new(pool),
        })
    }

    /// Number of worker threads in the pool.
    pub fn num_threads(&self) -> usize {
        self.inner.current_num_threads()
    }

    /// Spawn a fire-and-forget background task. The task is not tracked; it
    /// completes on its own. Use [`spawn_periodic`](Self::spawn_periodic) for
    /// work that must be stoppable or joinable.
    pub fn spawn<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.inner.spawn(task);
    }

    /// Run `task` on the pool and block until it returns.
    ///
    /// The pool's worker threads execute `task` (which may itself fan out
    /// with rayon `par_iter`), so heavy parallel passes like table flush run
    /// on the same bounded worker set as the background maintenance tasks.
    pub fn install<R, F>(&self, task: F) -> R
    where
        R: Send,
        F: FnOnce() -> R + Send,
    {
        self.inner.install(task)
    }

    /// Spawn a periodic task driven by the shared `running` flag.
    ///
    /// The loop runs on a dedicated driver thread: each pass executes `task`
    /// until `running` is cleared (via the returned handle or by the caller),
    /// then sleeps so the total cycle lasts at least `interval` (bounded
    /// below by `min_interval`). Heavy work inside `task` should fan out
    /// through [`Self::install`] so it still uses the shared pool.
    pub fn spawn_periodic<F>(
        &self,
        running: Arc<AtomicBool>,
        interval: Duration,
        min_interval: Duration,
        task: F,
    ) -> BackgroundTaskHandle
    where
        F: FnMut() + Send + 'static,
    {
        running.store(true, Ordering::Release);
        let finished = Arc::new(Mutex::new(false));
        let condvar = Arc::new(Condvar::new());

        let mut task = task;
        let run = running.clone();
        let fin = finished.clone();
        let cv = condvar.clone();
        let driver = move || {
            while run.load(Ordering::Acquire) {
                let start = std::time::Instant::now();
                task();
                if !run.load(Ordering::Acquire) {
                    break;
                }
                let sleep = interval.saturating_sub(start.elapsed()).max(min_interval);
                std::thread::sleep(sleep);
            }
            *fin.lock() = true;
            cv.notify_all();
        };
        if std::thread::Builder::new()
            .name("linkrs-storage-periodic".to_string())
            .spawn(driver)
            .is_err()
        {
            log::warn!("Failed to spawn periodic driver thread; loop left stopped");
            running.store(false, Ordering::Release);
            *finished.lock() = true;
            condvar.notify_all();
        }

        BackgroundTaskHandle {
            running,
            finished,
            condvar,
        }
    }
}

impl Default for StorageThreadPool {
    fn default() -> Self {
        Self::new().expect("storage thread pool construction cannot fail with defaults")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_spawn_one_shot() {
        let pool = StorageThreadPool::new().unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        pool.spawn(move || {
            f.store(true, Ordering::Release);
        });
        // Poll briefly; the task runs on the pool.
        for _ in 0..100 {
            if flag.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("one-shot task did not run");
    }

    #[test]
    fn test_spawn_periodic_stop_and_join() {
        let pool = StorageThreadPool::new().unwrap();
        let running = Arc::new(AtomicBool::new(false));
        let passes = Arc::new(AtomicUsize::new(0));
        let p = passes.clone();
        let handle = pool.spawn_periodic(
            running.clone(),
            Duration::from_millis(10),
            Duration::from_millis(1),
            move || {
                p.fetch_add(1, Ordering::Relaxed);
            },
        );

        std::thread::sleep(Duration::from_millis(50));
        assert!(passes.load(Ordering::Relaxed) > 0);

        handle.stop();
        handle.join();
        let count_after_stop = passes.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            passes.load(Ordering::Relaxed),
            count_after_stop,
            "loop must stop after stop()"
        );
    }

    #[test]
    fn test_pool_shared_across_clones() {
        let pool = StorageThreadPool::new().unwrap();
        let pool2 = pool.clone();
        let pool3 = pool.clone();
        assert_eq!(
            pool.inner.current_num_threads(),
            pool2.inner.current_num_threads()
        );
        assert_eq!(
            pool2.inner.current_num_threads(),
            pool3.inner.current_num_threads()
        );
    }
}
