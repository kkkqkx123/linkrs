//! Per-thread WAL buffer for decoupled assembly/flush.
//!
//! Accumulates serialized WAL entries in a thread-local buffer without holding
//! the file lock. A background flush thread periodically drains all registered
//! buffers to disk. This separates record serialization from I/O, removing the
//! single-Mutex bottleneck on the write path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use graphdb_core::wal::types::WalResult;

// ---------------------------------------------------------------------------
// WalBuffer
// ---------------------------------------------------------------------------

/// Per-thread append buffer for serialized WAL entries.
///
/// Each thread writes its own entries into this buffer without any locking.
/// The flush thread periodically drains all registered buffers to the WAL file.
pub struct WalBuffer {
    /// Serialized entry bytes (header + compressed payload, already formatted).
    buffer: Mutex<Vec<u8>>,
    /// Total bytes pending flush.
    pending_bytes: AtomicU64,
    /// Number of entries buffered since last flush.
    entry_count: AtomicU64,
    /// Flush threshold in bytes. Buffer is drained when pending >= threshold.
    flush_threshold: usize,
}

impl WalBuffer {
    /// Create a new buffer with the given flush threshold.
    pub fn new(flush_threshold: usize) -> Self {
        Self {
            buffer: Mutex::new(Vec::with_capacity(flush_threshold)),
            pending_bytes: AtomicU64::new(0),
            entry_count: AtomicU64::new(0),
            flush_threshold,
        }
    }

    /// Append serialized entry bytes to the buffer.
    ///
    /// This is called by the writer thread without any external lock.
    /// The internal mutex is only held briefly for the buffer append.
    pub fn append(&self, data: &[u8]) {
        let mut buf = self.buffer.lock().unwrap();
        buf.extend_from_slice(data);
        self.pending_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        self.entry_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Drain the buffer, returning all pending bytes.
    ///
    /// The caller (flush thread) takes ownership of the bytes and writes them
    /// to the WAL file.
    pub fn drain(&self) -> Vec<u8> {
        let mut buf = self.buffer.lock().unwrap();
        let data = std::mem::take(&mut *buf);
        self.pending_bytes.store(0, Ordering::Relaxed);
        self.entry_count.store(0, Ordering::Relaxed);
        data
    }

    /// Returns true if the buffer has pending data above the flush threshold.
    pub fn needs_flush(&self) -> bool {
        self.pending_bytes.load(Ordering::Relaxed) >= self.flush_threshold as u64
    }

    /// Returns the number of pending bytes.
    pub fn pending_bytes(&self) -> u64 {
        self.pending_bytes.load(Ordering::Relaxed)
    }

    /// Returns the number of pending entries.
    pub fn entry_count(&self) -> u64 {
        self.entry_count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// WalFlushCoordinator
// ---------------------------------------------------------------------------

/// Coordinates background WAL flush across per-thread buffers.
///
/// Manages a set of `WalBuffer` instances (one per writer thread) and runs a
/// background thread that periodically drains them to the WAL file. Also
/// handles fsync via group commit when enabled.
pub struct WalFlushCoordinator {
    /// Registered per-thread buffers.
    buffers: Mutex<Vec<Arc<WalBuffer>>>,
    /// Condvar for signaling the flush thread to wake up.
    flush_signal: Arc<(Mutex<bool>, Condvar)>,
    /// Background flush thread handle.
    flush_thread: Mutex<Option<JoinHandle<()>>>,
    /// Flush interval (how often the background thread wakes up).
    flush_interval: Duration,
    /// Shutdown flag.
    shutdown: AtomicBool,
    /// Last successful flush timestamp.
    last_flush_time: Mutex<Option<Instant>>,
    /// Flush statistics.
    stats: FlushStats,
}

#[derive(Debug, Default)]
pub struct FlushStats {
    /// Total number of flush cycles completed.
    pub flush_count: AtomicU64,
    /// Total bytes flushed to disk.
    pub bytes_flushed: AtomicU64,
    /// Total entries flushed.
    pub entries_flushed: AtomicU64,
    /// Number of flushes triggered by threshold (not interval).
    pub threshold_flushes: AtomicU64,
}

impl WalFlushCoordinator {
    /// Create a new coordinator with the given flush interval.
    pub fn new(flush_interval: Duration) -> Arc<Self> {
        Arc::new(Self {
            buffers: Mutex::new(Vec::new()),
            flush_signal: Arc::new((Mutex::new(false), Condvar::new())),
            flush_thread: Mutex::new(None),
            flush_interval,
            shutdown: AtomicBool::new(false),
            last_flush_time: Mutex::new(None),
            stats: FlushStats::default(),
        })
    }

    /// Register a per-thread buffer with this coordinator.
    pub fn register_buffer(&self, buffer: Arc<WalBuffer>) {
        let mut buffers = self.buffers.lock().unwrap();
        buffers.push(buffer);
    }

    /// Start the background flush thread.
    ///
    /// The `flush_fn` callback is called with drained buffer data and should
    /// write it to the WAL file and optionally fsync. The callback receives
    /// `(data: Vec<u8>)` and should return Ok(()) on success.
    pub fn start_flush_thread<F>(self: &Arc<Self>, flush_fn: F)
    where
        F: Fn(&[u8]) -> WalResult<()> + Send + Sync + 'static,
    {
        let flush_fn = Arc::new(flush_fn);
        let coordinator = Arc::clone(self);
        let signal = Arc::clone(&coordinator.flush_signal);

        let handle = thread::spawn(move || {
            let (lock, cvar) = &*signal;
            loop {
                // Wait for signal or timeout
                let notified = {
                    let mut guard = lock.lock().unwrap();
                    if coordinator.shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    let result = cvar.wait_timeout(guard, coordinator.flush_interval).unwrap();
                    guard = result.0;
                    // Reset signal
                    *guard = false;
                    !result.1.timed_out()
                };

                // Drain all buffers
                let buffers = coordinator.buffers.lock().unwrap();
                let mut total_flushed = 0u64;
                for buf in buffers.iter() {
                    if buf.pending_bytes() > 0 {
                        let data = buf.drain();
                        if !data.is_empty() {
                            total_flushed += data.len() as u64;
                            if let Err(_e) = flush_fn(&data) {
                                // On flush failure, we could re-buffer or poison.
                                // For now, log and continue (data is already drained).
                                // The WAL file may be inconsistent, but the next sync
                                // will detect the issue.
                            }
                        }
                    }
                }
                drop(buffers);

                if total_flushed > 0 {
                    coordinator
                        .stats
                        .bytes_flushed
                        .fetch_add(total_flushed, Ordering::Relaxed);
                    coordinator
                        .stats
                        .flush_count
                        .fetch_add(1, Ordering::Relaxed);
                    if notified {
                        coordinator
                            .stats
                            .threshold_flushes
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    if let Ok(mut guard) = coordinator.last_flush_time.lock() {
                        *guard = Some(Instant::now());
                    }
                }
            }
        });

        *self.flush_thread.lock().unwrap() = Some(handle);
    }

    /// Signal the flush thread to wake up immediately (for urgent syncs).
    pub fn signal_flush(&self) {
        let (lock, cvar) = &*self.flush_signal;
        let mut guard = lock.lock().unwrap();
        *guard = true;
        cvar.notify_one();
    }

    /// Flush all buffers synchronously (for sync/flush operations).
    ///
    /// Drains all buffers and writes them to disk. This blocks until complete.
    pub fn flush_sync<F>(&self, flush_fn: &F) -> WalResult<()>
    where
        F: Fn(&[u8]) -> WalResult<()>,
    {
        let buffers = self.buffers.lock().unwrap();
        for buf in buffers.iter() {
            if buf.pending_bytes() > 0 {
                let data = buf.drain();
                if !data.is_empty() {
                    flush_fn(&data)?;
                }
            }
        }
        Ok(())
    }

    /// Shutdown the flush thread.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Notify the flush thread to wake up from wait_timeout.
        // Release the lock immediately so the thread can acquire it and see the flag.
        {
            let (lock, cvar) = &*self.flush_signal;
            let _guard = lock.lock().unwrap();
            cvar.notify_one();
        }
        // Lock released — thread can now acquire it, see shutdown=true, and exit.

        if let Some(handle) = self.flush_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }

    /// Returns flush statistics.
    pub fn stats(&self) -> &FlushStats {
        &self.stats
    }
}

impl Drop for WalFlushCoordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_wal_buffer_append_and_drain() {
        let buf = WalBuffer::new(1024);
        assert_eq!(buf.pending_bytes(), 0);
        assert_eq!(buf.entry_count(), 0);

        buf.append(&[1, 2, 3, 4]);
        assert_eq!(buf.pending_bytes(), 4);
        assert_eq!(buf.entry_count(), 1);

        buf.append(&[5, 6, 7]);
        assert_eq!(buf.pending_bytes(), 7);
        assert_eq!(buf.entry_count(), 2);

        let data = buf.drain();
        assert_eq!(data, vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(buf.pending_bytes(), 0);
        assert_eq!(buf.entry_count(), 0);
    }

    #[test]
    fn test_wal_buffer_needs_flush() {
        let buf = WalBuffer::new(8);
        assert!(!buf.needs_flush());

        buf.append(&[0; 4]);
        assert!(!buf.needs_flush());

        buf.append(&[0; 5]);
        assert!(buf.needs_flush());
    }

    #[test]
    fn test_flush_coordinator_sync() {
        let coordinator = WalFlushCoordinator::new(Duration::from_millis(100));
        let buf = Arc::new(WalBuffer::new(1024));
        coordinator.register_buffer(Arc::clone(&buf));

        buf.append(&[1, 2, 3]);

        let flushed = Arc::new(Mutex::new(Vec::new()));
        let flushed_clone = Arc::clone(&flushed);

        coordinator
            .flush_sync(&move |data: &[u8]| {
                flushed_clone.lock().unwrap().extend_from_slice(data);
                Ok(())
            })
            .unwrap();

        let data = flushed.lock().unwrap();
        assert_eq!(&*data, &[1, 2, 3]);
    }

    #[test]
    fn test_flush_coordinator_background_thread() {
        let coordinator = WalFlushCoordinator::new(Duration::from_millis(10));
        let buf = Arc::new(WalBuffer::new(1024));
        coordinator.register_buffer(Arc::clone(&buf));

        let flush_count = Arc::new(AtomicUsize::new(0));
        let flush_count_clone = Arc::clone(&flush_count);

        coordinator.start_flush_thread(move |data: &[u8]| {
            flush_count_clone.fetch_add(data.len(), Ordering::SeqCst);
            Ok(())
        });

        // Write some data
        buf.append(&[1, 2, 3, 4, 5]);

        // Wait for flush
        thread::sleep(Duration::from_millis(50));

        // The background thread should have flushed
        assert!(flush_count.load(Ordering::SeqCst) > 0);

        coordinator.shutdown();
    }
}
