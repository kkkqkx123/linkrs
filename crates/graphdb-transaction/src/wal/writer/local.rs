//! Local file-based WAL writer
//!
//! Thin facade over the writer: state definition, construction, open/close,
//! append dispatch and read-only accessors live here. The async buffering
//! runtime lives in [`async_buffer`], durability decision paths in
//! [`durability`], file lifecycle and rotation in [`file_ops`], the file
//! header in [`header`], poison state in [`poison`], record assembly for
//! both synchronous and buffered append paths in [`record`], and group
//! commit enablement in [`sync`].

mod async_buffer;
mod durability;
mod file_ops;
mod header;
mod poison;
mod record;
mod sync;
#[cfg(test)]
mod tests;

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use graphdb_core::types::Timestamp;
use graphdb_core::wal::traits::WalWriter;
use graphdb_core::wal::types::{
    Lsn, RecordType, WalCompression, WalConfig, WalError, WalFileHeader, WalOpType, WalResult,
    WalStats,
};

use super::buffer;
use super::compression::{create_compressor, Compressor};
use super::group_commit::GroupCommitCoordinator;
use super::sync::elapsed_since;
use crate::wal::parser::{LocalWalParser, WalParser};

use async_buffer::BufferedFlushState;

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
        Self::with_config(wal_uri, thread_id, WalConfig::default())
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
        self.wait_durable_via_file(appended_lsn)
    }
}

impl LocalWalWriter {
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
}

impl Drop for LocalWalWriter {
    fn drop(&mut self) {
        self.close();
    }
}
