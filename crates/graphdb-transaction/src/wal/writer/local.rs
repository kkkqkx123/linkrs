//! Local file-based WAL writer

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::buffer;
use super::compression::{create_compressor, Compressor};
use super::group_commit::GroupCommitCoordinator;
use super::sync::elapsed_since;
use crate::wal::parser::{LocalWalParser, WalParser};
use graphdb_core::types::Timestamp;
use graphdb_core::wal::traits::WalWriter;
use graphdb_core::wal::types::{
    Lsn, RecordType, WalCompression, WalConfig, WalError, WalFileHeader, WalOpType, WalResult,
    WalStats, WAL_HEADER_SIZE, WAL_MAX_RECORD_SIZE,
};
mod file_ops;
mod header;
mod poison;
mod record;
mod sync;

pub(crate) struct WalHeaderParams<'a> {
    pub op_type: WalOpType,
    pub timestamp: Timestamp,
    pub payload_len: usize,
    pub prev_lsn: Lsn,
    pub new_lsn: Lsn,
    pub record_type: RecordType,
    pub payload: &'a [u8],
    pub compression: WalCompression,
}

/// File position owned by the async flush path.
///
/// The background flush thread and `sync(&self)` share this state through an
/// `Arc<Mutex<..>>` so buffered bytes can be drained to disk without `&mut`
/// access to the writer. It holds a cloned file handle plus the next write
/// offset; the synchronous `self.file`/`file_used` pair is synced back after
/// each drain.
pub(crate) struct BufferedFlushState {
    file: File,
    offset: u64,
    file_size: usize,
}
pub struct LocalWalWriter {
    wal_uri: String,
    thread_id: u32,
    file: Option<File>,
    file_path: Option<PathBuf>,
    file_size: usize,
    file_used: usize,
    version: u32,
    checkpoint_seq: u64,
    current_lsn: AtomicU64,
    last_synced_lsn: AtomicU64,
    file_start_lsn: Lsn,
    stats: WalStats,
    config: WalConfig,
    is_open: AtomicBool,
    file_header: Option<WalFileHeader>,
    compressor: Box<dyn Compressor>,
    write_count: AtomicU64,
    last_sync_time: Mutex<Option<Instant>>,
    poisoned: AtomicBool,
    poison_reason: Mutex<Option<String>>,
    group_commit: Option<GroupCommitCoordinator>,
    /// Optional per-thread buffer for async flush. When set, `append()` writes
    /// to this buffer instead of directly to the file. The flush coordinator
    /// drains the buffer periodically.
    buffer: Option<Arc<buffer::WalBuffer>>,
    /// Coordinates background flush of per-thread buffers. Sits above the
    /// group-commit coordinator: flush thread drains buffers to file, then
    /// group commit batches the fsync.
    flush_coordinator: Option<Arc<buffer::WalFlushCoordinator>>,
    /// Shared file position for the async flush path (see `BufferedFlushState`).
    flush_state: Option<Arc<Mutex<BufferedFlushState>>>,
    /// Latest LSN staged in the async buffer. Read by the flush thread to
    /// advance group commit after writing, without touching writer threads.
    buffered_lsn: Arc<AtomicU64>,
    /// Guards one-time background thread startup.
    background_started: Arc<AtomicBool>,
}

impl LocalWalWriter {
    pub fn new(wal_uri: &str, thread_id: u32) -> Self {
        let config = WalConfig::default();
        let compressor = create_compressor(&config);
        Self {
            wal_uri: wal_uri.to_string(),
            thread_id,
            file: None,
            file_path: None,
            file_size: 0,
            file_used: 0,
            version: 0,
            checkpoint_seq: 0,
            current_lsn: AtomicU64::new(0),
            last_synced_lsn: AtomicU64::new(0),
            file_start_lsn: Lsn::ZERO,
            stats: WalStats::new(),
            config,
            is_open: AtomicBool::new(false),
            file_header: None,
            compressor,
            write_count: AtomicU64::new(0),
            last_sync_time: Mutex::new(None),
            poisoned: AtomicBool::new(false),
            poison_reason: Mutex::new(None),
            group_commit: None,
            buffer: None,
            flush_coordinator: None,
            flush_state: None,
            buffered_lsn: Arc::new(AtomicU64::new(0)),
            background_started: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with custom configuration
    pub fn with_config(wal_uri: &str, thread_id: u32, config: WalConfig) -> Self {
        let compressor = create_compressor(&config);

        Self {
            wal_uri: wal_uri.to_string(),
            thread_id,
            file: None,
            file_path: None,
            file_size: 0,
            file_used: 0,
            version: 0,
            checkpoint_seq: 0,
            current_lsn: AtomicU64::new(0),
            last_synced_lsn: AtomicU64::new(0),
            file_start_lsn: Lsn::ZERO,
            stats: WalStats::new(),
            config,
            is_open: AtomicBool::new(false),
            file_header: None,
            compressor,
            write_count: AtomicU64::new(0),
            last_sync_time: Mutex::new(None),
            poisoned: AtomicBool::new(false),
            poison_reason: Mutex::new(None),
            group_commit: None,
            buffer: None,
            flush_coordinator: None,
            flush_state: None,
            buffered_lsn: Arc::new(AtomicU64::new(0)),
            background_started: Arc::new(AtomicBool::new(false)),
        }
    }
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
    fn ensure_flush_state(&mut self) {
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
    fn drain_buffer_to_state(&self) -> WalResult<bool> {
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

    /// File-level sync shared by `sync()` after buffers are drained.
    fn sync_via_file(&self) -> WalResult<()> {
        self.check_poisoned()?;
        let current_lsn = self.current_lsn.load(Ordering::SeqCst);
        if let Some(ref coordinator) = self.group_commit {
            coordinator.record_appended(current_lsn);
            coordinator.append_and_wait(current_lsn)?;
        } else if self.flush_state.is_some() {
            // Buffered mode without group commit: fsync via shared handle.
            if let Some(ref state) = self.flush_state {
                let guard = state.lock().map_err(|e| {
                    WalError::InvalidOperation(format!("flush state lock poisoned: {}", e))
                })?;
                guard.file.sync_all().map_err(|e| {
                    self.poison(format!("fsync failed: {}", e));
                    WalError::IoError(e.to_string())
                })?;
            }
        } else if let Some(ref file) = self.file {
            if let Err(e) = file.sync_all() {
                self.poison(format!("fsync failed: {}", e));
                return Err(WalError::IoError(e.to_string()));
            }
        }
        self.last_synced_lsn.store(current_lsn, Ordering::SeqCst);
        self.write_count.store(0, Ordering::SeqCst);
        if let Ok(mut guard) = self.last_sync_time.lock() {
            *guard = Some(Instant::now());
        }
        Ok(())
    }

    /// Returns a reference to the async buffer, if enabled.
    pub fn buffer(&self) -> Option<&Arc<buffer::WalBuffer>> {
        self.buffer.as_ref()
    }

    pub fn current_lsn(&self) -> Lsn {
        Lsn::new(self.current_lsn.load(Ordering::SeqCst))
    }

    pub fn last_synced_lsn(&self) -> Lsn {
        Lsn::new(self.last_synced_lsn.load(Ordering::SeqCst))
    }

    /// Get the latest LSN known to be durable according to the configured sync policy.
    pub fn durable_lsn(&self) -> Lsn {
        self.last_synced_lsn()
    }

    pub fn set_current_lsn(&self, lsn: Lsn) {
        let old = self.current_lsn.load(Ordering::SeqCst);
        log::error!(
            "WAL SET_CURRENT_LSN: old={}, new={}, delta={}",
            old,
            lsn.as_u64(),
            lsn.as_u64().saturating_sub(old)
        );
        self.current_lsn.store(lsn.as_u64(), Ordering::SeqCst);
    }

    pub fn file_size(&self) -> usize {
        self.file_size
    }

    pub fn file_used(&self) -> usize {
        if let Some(ref state) = self.flush_state {
            if let Ok(guard) = state.lock() {
                return (guard.offset as usize).max(self.file_used);
            }
        }
        self.file_used
    }

    pub fn get_stats(&self) -> &WalStats {
        &self.stats
    }

    pub fn reset_stats(&mut self) {
        self.stats = WalStats::new();
    }

    /// Append a WAL entry to the async buffer instead of directly to file.
    ///
    /// Builds the WAL header + compressed payload, serializes them to bytes,
    /// and appends to the per-thread buffer. The flush coordinator will drain
    /// the buffer to disk periodically.
    fn append_entry_buffered(
        &mut self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
    ) -> WalResult<()> {
        self.check_poisoned()?;
        if !self.is_open.load(Ordering::SeqCst) {
            return Err(WalError::Closed);
        }

        let buffer = self
            .buffer
            .as_ref()
            .ok_or_else(|| WalError::InvalidOperation("async buffer not enabled".to_string()))?;

        // Compress payload
        let (final_payload, compression) = self.compressor.compress(payload)?;

        if final_payload.len() > WAL_MAX_RECORD_SIZE {
            return self.append_fragmented_buffered(
                op_type,
                timestamp,
                &final_payload,
                compression,
            );
        }

        // Build WAL header
        let prev_lsn = Lsn::new(self.current_lsn.load(Ordering::SeqCst));
        let entry_size = WAL_HEADER_SIZE + final_payload.len();
        let new_lsn = Lsn::new(prev_lsn.as_u64() + entry_size as u64);

        let header = self.build_wal_header(WalHeaderParams {
            op_type,
            timestamp,
            payload_len: final_payload.len(),
            prev_lsn,
            new_lsn,
            record_type: RecordType::Full,
            payload: &final_payload,
            compression,
        });

        // Serialize header + payload to bytes. Entries are written
        // contiguously with no padding so the flushed byte stream is
        // identical to synchronous writes and recovery is unchanged.
        let header_bytes = header.as_bytes();
        let mut entry_bytes = Vec::with_capacity(header_bytes.len() + final_payload.len());
        entry_bytes.extend_from_slice(&header_bytes);
        entry_bytes.extend_from_slice(&final_payload);

        // Append to buffer (no file I/O)
        buffer.append(&entry_bytes);

        // Update LSN
        self.current_lsn.store(new_lsn.as_u64(), Ordering::SeqCst);
        self.buffered_lsn.store(new_lsn.as_u64(), Ordering::SeqCst);

        // Wake the flush thread when the buffer crosses its threshold.
        if buffer.needs_flush() {
            self.request_flush();
        }

        Ok(())
    }

    /// Buffered variant of fragmented writes: each fragment is serialized
    /// into the per-thread buffer with First/Middle/Last record types.
    fn append_fragmented_buffered(
        &mut self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
        compression: WalCompression,
    ) -> WalResult<()> {
        let buffer = self
            .buffer
            .as_ref()
            .ok_or_else(|| WalError::InvalidOperation("async buffer not enabled".to_string()))?;
        let total_chunks = payload.len().div_ceil(WAL_MAX_RECORD_SIZE);
        let mut offset = 0;
        let mut chunk_index = 0;
        while offset < payload.len() {
            let chunk_end = (offset + WAL_MAX_RECORD_SIZE).min(payload.len());
            let chunk_data = &payload[offset..chunk_end];
            let prev_lsn = Lsn::new(self.current_lsn.load(Ordering::SeqCst));
            let new_lsn = Lsn::new(prev_lsn.as_u64() + (WAL_HEADER_SIZE + chunk_data.len()) as u64);
            let record_type = if total_chunks == 1 {
                RecordType::Full
            } else if chunk_index == 0 {
                RecordType::First
            } else if chunk_index == total_chunks - 1 {
                RecordType::Last
            } else {
                RecordType::Middle
            };
            let header = self.build_wal_header(WalHeaderParams {
                op_type,
                timestamp,
                payload_len: chunk_data.len(),
                prev_lsn,
                new_lsn,
                record_type,
                payload: chunk_data,
                compression,
            });
            let header_bytes = header.as_bytes();
            let mut entry_bytes = Vec::with_capacity(header_bytes.len() + chunk_data.len());
            entry_bytes.extend_from_slice(&header_bytes);
            entry_bytes.extend_from_slice(chunk_data);
            buffer.append(&entry_bytes);
            self.current_lsn.store(new_lsn.as_u64(), Ordering::SeqCst);
            self.buffered_lsn.store(new_lsn.as_u64(), Ordering::SeqCst);
            offset = chunk_end;
            chunk_index += 1;
        }
        if buffer.needs_flush() {
            self.request_flush();
        }
        Ok(())
    }
}

impl WalWriter for LocalWalWriter {
    fn open(&mut self) -> WalResult<()> {
        self.check_poisoned()?;
        if self.is_open.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.version += 1;
        let path = self.find_available_path()?;

        if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(version_str) =
                file_name.strip_prefix(&format!("thread_{}_wal_", self.thread_id))
            {
                if let Ok(version) = u32::from_str_radix(version_str, 16) {
                    self.version = version;
                }
            }
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        // A new WAL segment continues the logical LSN range of existing
        // segments. Without this fence, reopening a writer resets LSN to zero
        // and recovery can reorder records from different files.
        if self.current_lsn.load(Ordering::SeqCst) == 0 {
            let mut parser = LocalWalParser::new();
            if parser.open(&self.wal_uri).is_ok() {
                let parsed_lsn = parser.last_lsn();
                log::error!(
                    "WAL OPEN PARSE: parsed_last_lsn={}, setting current_lsn",
                    parsed_lsn
                );
                self.current_lsn
                    .store(parsed_lsn.as_u64(), Ordering::SeqCst);
                self.last_synced_lsn
                    .store(parsed_lsn.as_u64(), Ordering::SeqCst);
            }
        }

        file.set_len(self.config.truncate_size as u64)?;

        self.file = Some(file);
        self.file_path = Some(path);
        self.file_size = self.config.truncate_size;
        self.file_used = 0;
        self.is_open.store(true, Ordering::SeqCst);

        self.write_file_header()?;

        // Initialize the shared async flush position when buffering is on.
        self.ensure_flush_state();
        self.buffered_lsn
            .store(self.current_lsn.load(Ordering::SeqCst), Ordering::SeqCst);

        Ok(())
    }

    fn close(&mut self) {
        if !self.is_open.swap(false, Ordering::SeqCst) {
            return;
        }

        // Drain pending buffered bytes so no staged entry is lost on close.
        // Crash-before-sync semantics still apply: only synced data is durable.
        let _ = self.flush_buffered_to_file();

        if let Some(ref coordinator) = self.flush_coordinator {
            coordinator.shutdown();
        }

        if let Some(ref file) = self.file {
            let _ = file.sync_all();
        }

        self.file = None;
        self.file_path = None;
        self.file_size = 0;
        self.file_used = 0;
        self.file_header = None;
        self.group_commit = None;
        self.flush_state = None;
        self.background_started.store(false, Ordering::SeqCst);
    }

    fn append(&mut self, data: &[u8]) -> WalResult<()> {
        self.check_poisoned()?;
        if !self.is_open.load(Ordering::SeqCst) {
            return Err(WalError::Closed);
        }

        self.rotate_if_needed()?;

        let file = self.file.as_mut().ok_or(WalError::Closed)?;

        let expected_size = self.file_used + data.len();
        if expected_size > self.file_size {
            let new_size =
                ((expected_size / self.config.truncate_size) + 1) * self.config.truncate_size;
            file.set_len(new_size as u64)?;
            self.file_size = new_size;
        }

        file.seek(SeekFrom::Start(self.file_used as u64))?;
        file.write_all(data)?;
        self.file_used += data.len();

        let new_lsn = self.current_lsn.load(Ordering::SeqCst) + data.len() as u64;
        self.current_lsn.store(new_lsn, Ordering::SeqCst);

        let write_count = self.write_count.fetch_add(1, Ordering::SeqCst) + 1;
        let elapsed = elapsed_since(*self.last_sync_time.lock().unwrap());
        if self.config.sync_policy.requires_sync(write_count, elapsed) {
            file.sync_data()?;
            let lsn = self.current_lsn.load(Ordering::SeqCst);
            self.last_synced_lsn.store(lsn, Ordering::SeqCst);
            self.write_count.store(0, Ordering::SeqCst);
            if let Ok(mut guard) = self.last_sync_time.lock() {
                *guard = Some(Instant::now());
            }
        }

        Ok(())
    }

    fn append_entry(
        &mut self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
    ) -> WalResult<()> {
        if self.buffer.is_some() {
            // Buffer mode: serialize entry to buffer instead of direct file write.
            self.append_entry_buffered(op_type, timestamp, payload)
        } else {
            LocalWalWriter::append_entry(self, op_type, timestamp, payload)
        }
    }

    fn sync(&self) -> WalResult<()> {
        // Buffered mode: drain staged bytes to the file before fsync so
        // entries are durable after `sync()` returns.
        if self.buffer.is_some() {
            self.drain_buffer_to_state()?;
        }
        self.sync_via_file()
    }

    fn wait_for_durable(&self, appended_lsn: u64) -> WalResult<()> {
        // Same drain-then-wait pattern as `sync()`: the flush path advances
        // group commit after writing, then we wait for fsync coverage.
        if self.buffer.is_some() {
            self.drain_buffer_to_state()?;
        }
        if let Some(ref coordinator) = self.group_commit {
            coordinator.record_appended(appended_lsn);
            coordinator.append_and_wait(appended_lsn)
        } else if self.flush_state.is_some() {
            self.check_poisoned()?;
            if let Some(ref state) = self.flush_state {
                let guard = state.lock().map_err(|e| {
                    WalError::InvalidOperation(format!("flush state lock poisoned: {}", e))
                })?;
                guard
                    .file
                    .sync_all()
                    .map_err(|e| WalError::IoError(e.to_string()))?;
            }
            self.last_synced_lsn.store(appended_lsn, Ordering::SeqCst);
            Ok(())
        } else if let Some(ref file) = self.file {
            self.check_poisoned()?;
            file.sync_all()
                .map_err(|e| WalError::IoError(e.to_string()))?;
            self.last_synced_lsn.store(appended_lsn, Ordering::SeqCst);
            Ok(())
        } else {
            Err(WalError::Closed)
        }
    }
}

impl Drop for LocalWalWriter {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::{
        collect_committed_transactions, LocalWalParser, SyncPolicy, TransactionWalEntry, WalParser,
    };
    use graphdb_core::types::{
        IdempotencyKey, IndexGeneration, OrderingKey, TargetId, TransactionId, VertexId,
    };
    use graphdb_core::wal::types::{
        ArchiveMode, WalHeader, WAL_FILE_HEADER_SIZE, WAL_MAX_RECORD_SIZE,
    };
    use graphdb_core::wal::{
        EntityRef, IndexMutation, IndexOperation, OutboxIntent, WAL_SYNC_WIRE_VERSION,
    };
    use tempfile::TempDir;

    #[test]
    fn test_local_wal_writer() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().expect("Failed to open WAL");

        assert!(writer.file_header().is_some());
        let header = writer.file_header().unwrap();
        assert!(header.is_valid());

        let header = WalHeader::new(WalOpType::InsertVertex, 1, 5);
        let mut data = header.as_bytes().to_vec();
        data.extend_from_slice(b"hello");

        writer.append(&data).expect("Failed to append");

        writer.sync().expect("Failed to sync");
        writer.close();
    }

    #[test]
    fn test_append_entry_with_checksum() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new().with_checksum(true);
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");

        writer
            .append_entry(WalOpType::InsertVertex, 1, b"payload")
            .expect("Failed to append entry");

        assert!(writer.file_used() > WAL_FILE_HEADER_SIZE);
        writer.close();
    }

    #[test]
    fn transaction_batch_returns_commit_record_end_lsn() {
        let temp_dir = TempDir::new().expect("temporary directory should be created");
        let wal_path = temp_dir.path().to_string_lossy().to_string();
        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().expect("WAL should open");
        let transaction_id = TransactionId::new(9);
        let intent = OutboxIntent {
            wire_version: WAL_SYNC_WIRE_VERSION,
            transaction_id,
            intent_sequence: 0,
            mutation: IndexMutation {
                wire_version: WAL_SYNC_WIRE_VERSION,
                target: TargetId::new("fulltext").expect("target should be valid"),
                index_id: 1,
                index_generation: IndexGeneration::new(1),
                entity_ref: EntityRef::Vertex(VertexId::try_from_int64(1).expect("test vertex id")),
                operation: IndexOperation::Upsert,
                document_or_vector: vec![1],
                idempotency_key: IdempotencyKey::new("txn-9:0")
                    .expect("idempotency key should be valid"),
                ordering_key: OrderingKey::new("index-1:vertex-1")
                    .expect("ordering key should be valid"),
            },
        };
        let commit_lsn = writer
            .append_transaction_batch(
                transaction_id,
                vec![TransactionWalEntry {
                    op_type: WalOpType::InsertVertex,
                    timestamp: 3,
                    payload: vec![4, 5, 6],
                    transaction_id: None,
                    mutation_sequence: None,
                }],
                &[intent],
            )
            .expect("transaction batch should append");
        assert_eq!(commit_lsn.get(), writer.current_lsn().as_u64());
        assert_eq!(commit_lsn.get(), writer.last_synced_lsn().as_u64());
        writer.close();

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("WAL should parse");
        let transactions = collect_committed_transactions(&parser.parse_all_entries())
            .expect("committed transaction should validate");
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].transaction_id, transaction_id);
        assert_eq!(transactions[0].commit_lsn, commit_lsn);
        assert_eq!(transactions[0].redo_entries.len(), 1);
        assert_eq!(transactions[0].intents.len(), 1);
    }

    #[test]
    fn test_append_batch() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().expect("Failed to open WAL");

        let entries: Vec<(WalOpType, Timestamp, &[u8])> = vec![
            (WalOpType::InsertVertex, 1, b"vertex1"),
            (WalOpType::InsertVertex, 2, b"vertex2"),
            (WalOpType::InsertEdge, 3, b"edge1"),
        ];

        writer
            .append_batch(&entries)
            .expect("Failed to append batch");
        writer.close();
    }

    #[test]
    fn test_wal_file_header() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let mut writer = LocalWalWriter::new(&wal_path, 42);
        writer.open().expect("Failed to open WAL");

        let header = writer.file_header().expect("No file header");
        assert!(header.is_valid());
        assert_eq!(header.thread_id, 42);
        assert_eq!(header.checkpoint_seq, 0);

        writer.close();
    }

    #[test]
    fn test_set_checkpoint_seq_updates_open_file_header() {
        use std::io::Read;

        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().expect("Failed to open WAL");

        writer
            .set_checkpoint_seq(7)
            .expect("Failed to update checkpoint seq");

        let file_path = writer
            .file_path
            .as_ref()
            .expect("WAL file path should exist")
            .clone();
        let mut file = std::fs::File::open(&file_path).expect("Failed to open WAL file");
        let mut buffer = [0u8; WAL_FILE_HEADER_SIZE];
        file.read_exact(&mut buffer)
            .expect("Failed to read WAL header");

        let header = WalFileHeader::from_bytes(&buffer).expect("Failed to parse WAL header");
        assert_eq!(header.checkpoint_seq, 7);

        writer.close();
    }

    #[test]
    fn test_truncate_reclaims_old_wal_files() {
        use std::io::Write;

        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().expect("Failed to open WAL");

        writer
            .append_entry(WalOpType::InsertVertex, 1, b"payload")
            .expect("Failed to append entry");

        let old_file_path = writer.get_wal_file_path(0);
        let old_header = WalFileHeader::new(0, 0, Lsn::ZERO);
        let mut old_file = std::fs::File::create(&old_file_path).expect("Failed to create WAL");
        old_file
            .write_all(&old_header.as_bytes())
            .expect("Failed to write WAL header");
        old_file
            .write_all(b"stale")
            .expect("Failed to write stale WAL data");

        let current_lsn = writer.current_lsn();
        writer
            .set_checkpoint_seq(1)
            .expect("Failed to update checkpoint seq");

        let deleted = writer
            .truncate(current_lsn)
            .expect("Failed to reclaim old WAL files");

        assert_eq!(deleted, 1);
        assert!(!old_file_path.exists());
        assert!(writer
            .file_path
            .as_ref()
            .expect("WAL file path should exist")
            .exists());

        writer.close();
    }

    #[test]
    fn test_lsn_tracking() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new()
            .with_checksum(true)
            .with_sync_policy(SyncPolicy::EveryWrite);
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");

        let initial_lsn = writer.current_lsn();

        writer
            .append_entry(WalOpType::InsertVertex, 1, b"payload1")
            .expect("Failed to append entry");

        let lsn_after_first = writer.current_lsn();
        assert!(lsn_after_first > initial_lsn);

        writer
            .append_entry(WalOpType::InsertVertex, 2, b"payload2")
            .expect("Failed to append entry");

        let lsn_after_second = writer.current_lsn();
        assert!(lsn_after_second > lsn_after_first);

        assert_eq!(writer.current_lsn(), writer.last_synced_lsn());

        writer.close();
    }

    #[test]
    fn test_sync_policy_batch() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new()
            .with_checksum(true)
            .with_sync_policy(SyncPolicy::Batch { batch_size: 3 });
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");

        writer
            .append_entry(WalOpType::InsertVertex, 1, b"payload1")
            .expect("Failed to append entry");
        assert_ne!(writer.current_lsn(), writer.last_synced_lsn());

        writer
            .append_entry(WalOpType::InsertVertex, 2, b"payload2")
            .expect("Failed to append entry");
        assert_ne!(writer.current_lsn(), writer.last_synced_lsn());

        writer
            .append_entry(WalOpType::InsertVertex, 3, b"payload3")
            .expect("Failed to append entry");
        assert_eq!(writer.current_lsn(), writer.last_synced_lsn());

        writer.close();
    }

    #[test]
    fn test_sync_policy_never() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new()
            .with_checksum(true)
            .with_sync_policy(SyncPolicy::Never);
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");

        for i in 0..10 {
            writer
                .append_entry(WalOpType::InsertVertex, i, b"payload")
                .expect("Failed to append entry");
        }

        assert_ne!(writer.current_lsn(), writer.last_synced_lsn());
        assert_eq!(writer.durable_lsn(), writer.last_synced_lsn());

        let pending_lsn = writer.current_lsn();
        assert!(writer.truncate(pending_lsn).is_err());

        writer.sync().expect("Failed to sync");
        assert_eq!(writer.current_lsn(), writer.last_synced_lsn());

        writer.close();
    }

    #[test]
    fn test_fragmented_entry() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new().with_checksum(true);
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");

        let large_payload: Vec<u8> = (0..(WAL_MAX_RECORD_SIZE * 2 + 1000))
            .map(|i| (i % 256) as u8)
            .collect();

        writer
            .append_entry(WalOpType::InsertVertex, 1, &large_payload)
            .expect("Failed to append fragmented entry");

        writer.sync().expect("Failed to sync");
        writer.close();
    }

    #[test]
    fn test_wal_rotation_basic() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::default()
            .with_max_file_size(1024)
            .with_truncate_size(4096);

        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");

        let data = vec![0u8; 512];
        for _ in 0..3 {
            writer.append(&data).expect("Failed to append");
        }

        assert!(writer.version >= 2);
        writer.close();
    }

    #[test]
    fn test_wal_file_naming() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::default();
        let writer = LocalWalWriter::with_config(&wal_path, 0, config);

        let path = writer.get_wal_file_path(1);
        assert!(path.to_string_lossy().contains("wal_00000001"));

        let path = writer.get_wal_file_path(100);
        assert!(path.to_string_lossy().contains("wal_00000064"));
    }

    #[test]
    fn test_wal_archive() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();
        let archive_path = temp_dir.path().join("archive");

        let config = WalConfig::default()
            .with_archive_dir(archive_path.to_string_lossy().to_string())
            .with_archive_mode(ArchiveMode::Move);

        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");

        let test_file = temp_dir.path().join("wal_00000001");
        std::fs::write(&test_file, vec![0u8; 100]).expect("Failed to create test file");

        writer
            .archive_wal_file(&test_file, archive_path.to_string_lossy().as_ref())
            .expect("Failed to archive");

        assert!(!test_file.exists());
        assert!(archive_path.exists());
        writer.close();
    }

    #[test]
    fn test_wal_rotation_with_recovery() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::default()
            .with_max_file_size(1024)
            .with_checksum(true);

        {
            let mut writer = LocalWalWriter::with_config(&wal_path, 0, config.clone());
            writer.open().expect("Failed to open WAL");

            for i in 0..10 {
                let data = format!("Entry {}", i).into_bytes();
                writer.append(&data).expect("Failed to append");
            }

            writer.sync().expect("Failed to sync");
        }

        let wal_files = std::fs::read_dir(&wal_path)
            .expect("Failed to read WAL dir")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.contains("_wal_"))
                    .unwrap_or(false)
            })
            .count();

        assert!(wal_files >= 1);
    }

    #[test]
    fn test_wal_poison_blocks_writes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().expect("WAL should open");

        writer.poison("test poison".to_string());
        assert!(writer.is_poisoned());
        assert_eq!(writer.poison_reason(), Some("test poison".to_string()));

        let result = writer.append_entry(WalOpType::InsertVertex, 1, b"payload");
        assert!(matches!(result, Err(WalError::Poisoned(_))));

        writer.close();
    }

    #[test]
    fn test_wal_poison_idempotent() {
        let writer = LocalWalWriter::new("/tmp/nonexistent", 0);
        writer.poison("first".to_string());
        writer.poison("second".to_string());

        assert!(writer.is_poisoned());
        assert_eq!(writer.poison_reason(), Some("first".to_string()));
    }

    #[test]
    fn test_wal_poison_blocks_open() {
        let mut writer = LocalWalWriter::new("/tmp/nonexistent", 0);
        writer.poison("poisoned before open".to_string());

        assert!(writer.open().is_err());
    }

    #[test]
    fn test_recovery_baseline_updates_empty_segment_header() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().expect("WAL should open");

        let baseline = Lsn::new(1234);
        writer
            .set_recovery_baseline_lsn(baseline)
            .expect("baseline should be accepted for an empty segment");
        assert_eq!(writer.current_lsn(), baseline);
        assert_eq!(writer.durable_lsn(), baseline);
        assert_eq!(writer.file_start_lsn(), baseline);

        writer
            .append_entry(WalOpType::InsertVertex, 1, b"payload")
            .expect("append after recovery baseline should succeed");
        writer.sync().expect("WAL sync should succeed");
        assert!(writer.current_lsn() > baseline);
    }

    #[test]
    fn test_async_flush_disabled_preserves_sync_behavior() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new().with_async_flush(false);
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");
        writer.enable_async_flush();
        assert!(!writer.is_async_enabled());

        writer
            .append_entry(WalOpType::InsertVertex, 1, b"payload")
            .expect("Failed to append entry");
        writer.sync().expect("Failed to sync");
        let lsn = writer.current_lsn();
        assert_eq!(writer.last_synced_lsn(), lsn);
        writer.close();

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("WAL should parse");
        assert!(!parser.parse_all_entries().is_empty());
    }

    #[test]
    fn test_async_buffer_flush_correctness_and_recovery() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new()
            .with_async_flush(true)
            .with_buffer_size(4096);
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");
        writer.enable_async_flush();
        assert!(writer.is_async_enabled());

        for i in 0..10u64 {
            writer
                .append_entry(
                    WalOpType::InsertVertex,
                    i,
                    format!("payload-{}", i).as_bytes(),
                )
                .expect("Failed to append entry");
        }
        // Entries are staged without file I/O on the hot path.
        assert!(writer.buffer().unwrap().pending_bytes() > 0);
        writer.sync().expect("Failed to sync");
        assert_eq!(writer.buffer().unwrap().pending_bytes(), 0);
        assert_eq!(writer.current_lsn(), writer.last_synced_lsn());
        writer.close();

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("WAL should parse");
        let entries = parser.parse_all_entries();
        assert_eq!(entries.len(), 10);
    }

    #[test]
    fn test_async_transaction_batch_recovery() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new().with_async_flush(true);
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");
        writer.enable_async_flush();
        writer
            .start_background_flush()
            .expect("background flush starts");

        let transaction_id = TransactionId::new(7);
        let commit_lsn = writer
            .append_transaction_batch(
                transaction_id,
                vec![crate::wal::TransactionWalEntry {
                    op_type: WalOpType::InsertVertex,
                    timestamp: 3,
                    payload: vec![4, 5, 6],
                    transaction_id: None,
                    mutation_sequence: None,
                }],
                &[],
            )
            .expect("transaction batch should append");
        assert_eq!(commit_lsn.get(), writer.current_lsn().as_u64());
        writer.request_flush();
        writer.sync().expect("sync should drain");
        writer.close();

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("WAL should parse");
        let transactions = collect_committed_transactions(&parser.parse_all_entries())
            .expect("committed transaction should validate");
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].transaction_id, transaction_id);
        assert_eq!(transactions[0].commit_lsn, commit_lsn);
    }

    #[test]
    fn test_async_concurrent_buffer_drain() {
        use std::sync::{Arc, Barrier};
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let config = WalConfig::new().with_async_flush(true);
        let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
        writer.open().expect("Failed to open WAL");
        writer.enable_async_flush();
        let writer = Arc::new(parking_lot::Mutex::new(writer));
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for t in 0..4 {
            let w = Arc::clone(&writer);
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait();
                for i in 0..25u64 {
                    w.lock()
                        .append_entry(
                            WalOpType::InsertVertex,
                            i,
                            format!("t{}-{}", t, i).as_bytes(),
                        )
                        .expect("append should succeed");
                }
            }));
        }
        for h in handles {
            h.join().expect("thread should not panic");
        }
        let mut writer = writer.lock();
        writer.sync().expect("sync should drain");
        assert_eq!(writer.buffer().unwrap().pending_bytes(), 0);
        writer.close();

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("WAL should parse");
        assert_eq!(parser.parse_all_entries().len(), 100);
    }
}
