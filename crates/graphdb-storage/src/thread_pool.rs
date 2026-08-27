//! Unified background thread pool.
//!
//! All long-running background work in `graphdb-storage` is routed through a
//! single clone-able [`StorageThreadPool`] instead of ad-hoc `thread::spawn`
//! sites. This bounds the number of OS threads the storage engine spawns and
//! gives periodic loops a first-class handle for cooperative stop and join.
//!
//! Two flavors of work are supported:
//!
//! - [`StorageThreadPool::spawn`] — fire-and-forget one-shot tasks (e.g. the
//!   background freeze/maintenance pass).
//! - [`StorageThreadPool::spawn_periodic`] — loops that run until their
//!   [`BackgroundTaskHandle::stop`] is called, with a handle that supports
//!   blocking [`BackgroundTaskHandle::join`] for graceful shutdown.

use crate::core::{StorageError, StorageResult};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Handle to a periodic background task running on the shared pool.
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

/// Shared background thread pool.
///
/// Clones share the same underlying rayon worker threads. The pool is sized
/// from `available_parallelism` (clamped) so background work reuses a bounded
/// set of threads across the whole storage engine.
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
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|idx| format!("graphdb-storage-{idx}"))
            .build()
            .map_err(|e| {
                StorageError::storage_error(format!("failed to build thread pool: {e}"))
            })?;
        Ok(Self {
            inner: Arc::new(pool),
        })
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
    /// The task runs `task` once per pass until `running` is cleared (via the
    /// returned handle or by the caller). Each pass sleeps so that the total
    /// cycle lasts at least `interval` (bounded below by `min_interval`).
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
        self.inner.spawn(move || {
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
        });

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
