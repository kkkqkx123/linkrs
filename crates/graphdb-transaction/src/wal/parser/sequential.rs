//! Sequential local WAL parser with LSN point/range query interfaces.

use std::path::{Path, PathBuf};

use graphdb_core::types::Timestamp;
use graphdb_core::wal::types::{
    Lsn, WalError, WalFileHeader, WalHeader, WalRecoveryMode, WalResult, WAL_FILE_HEADER_SIZE,
    WAL_HEADER_SIZE,
};

use super::discover::{self, WalFileData};
use super::factory::WalParser;
use super::scan::parse_wal_file_bytes;
use super::types::{ParsedWalEntry, WalEntryIter};

/// Local file-based WAL parser
pub struct LocalWalParser {
    /// WAL directory path
    wal_dir: Option<PathBuf>,
    /// All parsed entries with LSN info
    all_entries: Vec<ParsedWalEntry>,
    /// Last seen timestamp
    last_timestamp: Timestamp,
    /// Last seen LSN
    last_lsn: Lsn,
    /// File headers for each parsed file
    file_headers: Vec<WalFileHeader>,
    /// Recovery mode
    recovery_mode: WalRecoveryMode,
    /// Enable checksum verification
    verify_checksum: bool,
    /// Number of corrupted entries found
    corrupted_count: usize,
    /// Number of skipped entries
    skipped_count: usize,
}

impl LocalWalParser {
    /// Create a new local WAL parser
    pub fn new() -> Self {
        Self {
            wal_dir: None,
            all_entries: Vec::new(),
            last_timestamp: 0,
            last_lsn: Lsn::ZERO,
            file_headers: Vec::new(),
            recovery_mode: WalRecoveryMode::default(),
            verify_checksum: true,
            corrupted_count: 0,
            skipped_count: 0,
        }
    }

    /// Create with custom recovery mode
    pub fn with_recovery_mode(recovery_mode: WalRecoveryMode) -> Self {
        Self {
            recovery_mode,
            verify_checksum: true,
            ..Self::new()
        }
    }

    /// Set checksum verification
    pub fn with_verify_checksum(mut self, verify: bool) -> Self {
        self.verify_checksum = verify;
        self
    }

    /// Get number of corrupted entries found
    pub fn corrupted_count(&self) -> usize {
        self.corrupted_count
    }

    /// Get number of skipped entries
    pub fn skipped_count(&self) -> usize {
        self.skipped_count
    }

    /// Get file headers
    pub fn file_headers(&self) -> &[WalFileHeader] {
        &self.file_headers
    }

    /// Parse all WAL files in the directory
    fn parse_wal_files(&mut self, wal_dir: &Path) -> WalResult<()> {
        let wal_files = discover::discover_wal_files(wal_dir, self.recovery_mode)?;

        for path in wal_files {
            if let Err(e) = self.parse_wal_file(&path) {
                match self.recovery_mode {
                    WalRecoveryMode::AbortOnCorruption => {
                        return Err(WalError::RecoveryAborted(format!(
                            "Failed to parse {}: {}",
                            path.display(),
                            e
                        )));
                    }
                    _ => {
                        self.corrupted_count += 1;
                        continue;
                    }
                }
            }
        }

        self.all_entries.sort_by_key(|e| e.lsn);

        Ok(())
    }

    /// Parse a single WAL file
    fn parse_wal_file(&mut self, path: &Path) -> WalResult<()> {
        let Some(data) = discover::read_wal_file(path, self.verify_checksum)? else {
            return Ok(());
        };

        log_file_debug(path, &data);
        self.file_headers.push(data.file_header);

        let result = parse_wal_file_bytes(
            &data.buffer,
            data.file_header.start_lsn(),
            self.recovery_mode,
            self.verify_checksum,
        )?;

        self.all_entries.extend(result.all_entries);
        self.corrupted_count += result.corrupted_count;
        self.skipped_count += result.skipped_count;
        self.last_timestamp = self.last_timestamp.max(result.last_timestamp);
        if result.last_lsn > self.last_lsn {
            self.last_lsn = result.last_lsn;
        }

        Ok(())
    }

    /// Get all WAL entries as an iterator
    pub fn iter_entries(&self) -> WalEntryIter<'_> {
        WalEntryIter::new(&self.all_entries)
    }

    /// Parse and return all entries with metadata
    pub fn parse_all_entries(&self) -> Vec<ParsedWalEntry> {
        self.all_entries.clone()
    }

    /// Get last LSN
    pub fn last_lsn(&self) -> Lsn {
        self.last_lsn
    }

    /// Get entry by LSN
    pub fn get_entry_by_lsn(&self, lsn: Lsn) -> Option<&ParsedWalEntry> {
        self.all_entries.iter().find(|e| e.lsn == lsn)
    }

    /// Get entries in LSN range
    pub fn get_entries_in_lsn_range(&self, start: Lsn, end: Lsn) -> Vec<&ParsedWalEntry> {
        self.all_entries
            .iter()
            .filter(|e| e.lsn >= start && e.lsn <= end)
            .collect()
    }

    /// Get all entries sorted by LSN
    pub fn get_all_entries_sorted_by_lsn(&self) -> Vec<&ParsedWalEntry> {
        let mut entries: Vec<_> = self.all_entries.iter().collect();
        entries.sort_by_key(|e| e.lsn);
        entries
    }
}

impl Default for LocalWalParser {
    fn default() -> Self {
        Self::new()
    }
}

impl WalParser for LocalWalParser {
    fn open(&mut self, wal_uri: &str) -> WalResult<()> {
        let wal_dir = PathBuf::from(wal_uri);
        self.wal_dir = Some(wal_dir.clone());
        self.parse_wal_files(&wal_dir)
    }

    fn close(&mut self) {
        self.all_entries.clear();
        self.file_headers.clear();
        self.last_timestamp = 0;
        self.last_lsn = Lsn::ZERO;
        self.corrupted_count = 0;
        self.skipped_count = 0;
    }

    fn last_timestamp(&self) -> Timestamp {
        self.last_timestamp
    }
}

fn log_file_debug(path: &Path, data: &WalFileData) {
    let file_start_lsn = data.file_header.start_lsn();
    log::error!(
        "WAL FILE DEBUG: path={:?}, file_size={}, file_start_lsn={}, checkpoint_seq={}, thread_id={}, first_record_at_64={:?}",
        path, data.buffer.len(), file_start_lsn, data.file_header.checkpoint_seq, data.file_header.thread_id,
        if data.buffer.len() >= WAL_FILE_HEADER_SIZE + WAL_HEADER_SIZE {
            WalHeader::from_bytes(&data.buffer[WAL_FILE_HEADER_SIZE..WAL_FILE_HEADER_SIZE + WAL_HEADER_SIZE])
                .map(|h| format!("prev_lsn={}, lsn={}, ts={}, len={}", h.prev_lsn(), h.lsn(), h.timestamp, h.length()))
                .unwrap_or_else(|| "invalid header".to_string())
        } else {
            "no records".to_string()
        }
    );
}
