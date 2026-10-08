//! Async buffering runtime: shared flush state, background flush thread
//! and buffer drain coordination.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use linkrs_core::wal::types::{WalError, WalResult};

use super::super::buffer;
use super::LocalWalWriter;

/// File position owned by the async flush path.
///
/// The background flush thread and `sync(&self)` share this state through an
/// `Arc<Mutex<..>>` so buffered bytes can be drained to disk without `&mut`
/// access to the writer. It holds a cloned file handle plus the next write
/// offset; the synchronous `self.file`/`file_used` pair is synced back after
/// each drain.
pub(crate) struct BufferedFlushState {
    pub(crate) file: File,
    pub(crate) offset: u64,
    pub(crate) file_size: usize,
}

impl LocalWalWriter {
    /// Enable async buffer mode for this writer.
    ///
    /// When enabled, `append_entry` serializes into a per-thread buffer
    /// without holding the file lock. The flush coordinator drains buffers
    /// to disk periodically or on explicit sync.
    pub fn enable_async_buffer(&mut self, buffer: Arc<buffer::WalBuffer>) {
        if self.flush_coordinator.is_none() {
            let coordinator = buffer::WalFlushCoordinator::new(self.config.flush_interval());
            coordinator.register_buffer(Arc::clone(&buffer));
            self.flush_coordinator = Some(coordinator);
        } else if let Some(ref coordinator) = self.flush_coordinator {
            coordinator.register_buffer(Arc::clone(&buffer));
        }
        self.buffer = Some(buffer);
        self.buffered_lsn
            .store(self.current_lsn.load(Ordering::SeqCst), Ordering::SeqCst);
        // Best-effort shared flush position when the file is already open.
        self.ensure_flush_state();
    }

    /// Enable async flush using config defaults (buffer size + interval).
    /// When `enable_async_flush` is false in config this is a no-op that
    /// preserves exact synchronous behavior.
    pub fn enable_async_flush(&mut self) {
        if !self.config.enable_async_flush {
            return;
        }
        let buf = Arc::new(buffer::WalBuffer::new(self.config.buffer_size));
        self.enable_async_buffer(buf);
    }

    /// Disable async mode and synchronously drain pending buffered data.
    pub fn disable_async_flush(&mut self) -> WalResult<()> {
        self.flush_buffered_to_file()?;
        self.buffer = None;
        if let Some(ref coordinator) = self.flush_coordinator {
            coordinator.shutdown();
        }
        self.flush_coordinator = None;
        self.flush_state = None;
        self.background_started.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Returns true when async buffer mode is active.
    pub fn is_async_enabled(&self) -> bool {
        self.buffer.is_some() && self.config.enable_async_flush
    }

    /// Request an immediate background flush wake-up.
    pub fn request_flush(&self) {
        if let Some(ref coordinator) = self.flush_coordinator {
            coordinator.signal_flush();
        }
    }

    /// Initialize the shared flush position from the open file handle.
    pub(crate) fn ensure_flush_state(&mut self) {
        if self.flush_state.is_some() || self.buffer.is_none() {
            return;
        }
        if let Some(ref file) = self.file {
            if let Ok(cloned) = file.try_clone() {
                self.flush_state = Some(Arc::new(Mutex::new(BufferedFlushState {
                    file: cloned,
                    offset: self.file_used as u64,
                    file_size: self.file_size,
                })));
            }
        }
    }

    /// Write drained bytes at the shared flush offset, growing the file.
    fn write_to_flush_state(state: &mut BufferedFlushState, data: &[u8]) -> WalResult<()> {
        let end = state.offset + data.len() as u64;
        if end as usize > state.file_size {
            state.file.set_len(end)?;
            state.file_size = end as usize;
        }
        state.file.seek(SeekFrom::Start(state.offset))?;
        state.file.write_all(data)?;
        state.offset = end;
        Ok(())
    }

    /// Drain one buffer into the shared flush position. Callable from `&self`
    /// so both the background thread and `sync()` can flush without `&mut`.
    pub(crate) fn drain_buffer_to_state(&self) -> WalResult<bool> {
        let buf = match self.buffer.as_ref() {
            Some(b) => Arc::clone(b),
            None => return Ok(false),
        };
        if buf.pending_bytes() == 0 {
            return Ok(false);
        }
        let state = match self.flush_state.as_ref() {
            Some(s) => Arc::clone(s),
            None => return Ok(false),
        };
        let data = buf.drain();
        if data.is_empty() {
            return Ok(false);
        }
        let mut guard = state
            .lock()
            .map_err(|e| WalError::InvalidOperation(format!("flush state lock poisoned: {}", e)))?;
        Self::write_to_flush_state(&mut guard, &data)?;
        // The flush path (not writer threads) advances group commit.
        let staged = self.buffered_lsn.load(Ordering::SeqCst);
        drop(guard);
        if let Some(ref coordinator) = self.group_commit {
            coordinator.record_appended(staged);
        }
        Ok(true)
    }

    /// Sync the synchronous file position from the shared flush offset.
    fn syncback_file_used(&mut self) {
        if let Some(ref state) = self.flush_state {
            if let Ok(guard) = state.lock() {
                self.file_used = guard.offset as usize;
                self.file_size = guard.file_size;
            }
        }
    }

    /// Drain all buffered bytes to the WAL file without fsync.
    pub fn flush_buffered_to_file(&mut self) -> WalResult<()> {
        if self.buffer.is_none() {
            return Ok(());
        }
        self.ensure_flush_state();
        // Fast path: shared state drain (also advances group commit).
        if self.flush_state.is_some() {
            let _ = self.drain_buffer_to_state()?;
            self.syncback_file_used();
            self.request_flush();
            return Ok(());
        }
        // Fallback when the file is not open yet: keep bytes buffered.
        Ok(())
    }

    /// Drain buffers and fsync, advancing the durable sequence.
    pub fn flush_and_sync(&mut self) -> WalResult<()> {
        self.flush_buffered_to_file()?;
        self.sync_via_file()
    }

    /// Start the background flush thread.
    ///
    /// The thread periodically drains registered buffers into the WAL file
    /// and advances group commit via `record_appended`, so writer threads
    /// never block on file I/O. Durability still requires `sync()` /
    /// `wait_for_durable()`, which drain synchronously first.
    pub fn start_background_flush(&mut self) -> WalResult<()> {
        if !self.is_async_enabled() {
            return Err(WalError::InvalidOperation(
                "async buffer not enabled".to_string(),
            ));
        }
        if self
            .background_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        self.ensure_flush_state();
        let coordinator = self.flush_coordinator.clone().ok_or_else(|| {
            WalError::InvalidOperation("flush coordinator not enabled".to_string())
        })?;
        let state = self
            .flush_state
            .clone()
            .ok_or_else(|| WalError::InvalidOperation("flush state not initialized".to_string()))?;
        let staged = Arc::clone(&self.buffered_lsn);
        let group = self.group_commit.clone();
        coordinator.start_flush_thread(move |data: &[u8]| {
            let mut guard = state.lock().map_err(|e| {
                WalError::InvalidOperation(format!("flush state lock poisoned: {}", e))
            })?;
            Self::write_to_flush_state(&mut guard, data)?;
            let lsn = staged.load(Ordering::SeqCst);
            drop(guard);
            if let Some(ref coordinator) = group {
                coordinator.record_appended(lsn);
            }
            Ok(())
        });
        Ok(())
    }

    /// Returns a reference to the async buffer, if enabled.
    pub fn buffer(&self) -> Option<&Arc<buffer::WalBuffer>> {
        self.buffer.as_ref()
    }
}
