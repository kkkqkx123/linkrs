//! WAL Parser
//!
//! Provides Write-Ahead Log parsing functionality for recovery

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::core::wal::types::{
    Lsn, RecordType, UpdateWalUnit, WalCompression, WalError, WalFileHeader, WalHeader,
    WalRecoveryMode, WalResult, WAL_FILE_HEADER_SIZE, WAL_HEADER_SIZE,
};
use crate::core::types::Timestamp;

/// WAL parser trait
pub trait WalParser: Send + Sync {
    /// Open and parse WAL files
    fn open(&mut self, wal_uri: &str) -> WalResult<()>;

    /// Close the parser
    fn close(&mut self);

    /// Get the last timestamp
    fn last_timestamp(&self) -> Timestamp;

    /// Get all update WAL units
    fn get_update_wals(&self) -> &[UpdateWalUnit];
}

/// Recovery result from parsing WAL files
#[derive(Debug, Default, Clone)]
pub struct RecoveryResult {
    /// All parsed entries with LSN info (primary recovery path)
    pub all_entries: Vec<ParsedWalEntry>,
    /// Last seen timestamp
    pub last_timestamp: Timestamp,
    /// Last seen LSN
    pub last_lsn: Lsn,
    /// Number of corrupted entries found
    pub corrupted_count: usize,
    /// Number of skipped entries
    pub skipped_count: usize,
}

/// Parallel WAL parser for faster recovery
pub struct ParallelWalParser {
    /// Number of threads to use
    num_threads: usize,
    /// Recovery mode
    recovery_mode: WalRecoveryMode,
    /// Enable checksum verification
    verify_checksum: bool,
}

impl ParallelWalParser {
    /// Create a new parallel WAL parser
    pub fn new() -> Self {
        Self {
            num_threads: num_cpus::get().max(1),
            recovery_mode: WalRecoveryMode::default(),
            verify_checksum: true,
        }
    }

    /// Set number of threads
    pub fn with_threads(mut self, num_threads: usize) -> Self {
        self.num_threads = num_threads.max(1);
        self
    }

    /// Set recovery mode
    pub fn with_recovery_mode(mut self, recovery_mode: WalRecoveryMode) -> Self {
        self.recovery_mode = recovery_mode;
        self
    }

    /// Set checksum verification
    pub fn with_verify_checksum(mut self, verify: bool) -> Self {
        self.verify_checksum = verify;
        self
    }

    /// Parse WAL files in parallel
    pub fn parse_parallel(&self, wal_dir: &Path) -> WalResult<RecoveryResult> {
        if !wal_dir.exists() {
            if self.recovery_mode == WalRecoveryMode::ErrorIfMissing {
                return Err(WalError::FileNotFound(
                    wal_dir.to_string_lossy().to_string(),
                ));
            }
            std::fs::create_dir_all(wal_dir).map_err(|e| WalError::IoError(e.to_string()))?;
            return Ok(RecoveryResult::default());
        }

        let mut wal_files: Vec<PathBuf> = std::fs::read_dir(wal_dir)
            .map_err(|e| WalError::IoError(e.to_string()))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "wal")
                    || path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("thread_") && n.contains("_wal_"))
            })
            .collect();

        wal_files.sort();

        if wal_files.is_empty() && self.recovery_mode == WalRecoveryMode::ErrorIfMissing {
            return Err(WalError::FileNotFound("No WAL files found".to_string()));
        }

        let results: Arc<Mutex<Vec<RecoveryResult>>> =
            Arc::new(Mutex::new(Vec::with_capacity(wal_files.len())));

        let recovery_mode = self.recovery_mode;
        let verify_checksum = self.verify_checksum;

        if wal_files.len() <= self.num_threads {
            for path in wal_files {
                let result = self.parse_single_file(&path, recovery_mode, verify_checksum)?;
                results
                    .lock()
                    .map(|mut r| r.push(result))
                    .map_err(|e| WalError::IoError(format!("Failed to acquire lock: {}", e)))?;
            }
        } else {
            let chunk_size = wal_files.len().div_ceil(self.num_threads);
            let chunks: Vec<Vec<PathBuf>> =
                wal_files.chunks(chunk_size).map(|c| c.to_vec()).collect();

            let handles: Vec<_> = chunks
                .into_iter()
                .map(|chunk| {
                    let results = Arc::clone(&results);

                    std::thread::spawn(move || {
                        let mut local_results = Vec::new();
                        for path in chunk {
                            match Self::parse_file_static(&path, recovery_mode, verify_checksum) {
                                Ok(result) => local_results.push(result),
                                Err(e) => {
                                    if recovery_mode == WalRecoveryMode::AbortOnCorruption {
                                        return Err(e);
                                    }
                                }
                            }
                        }
                        if let Ok(mut r) = results.lock() {
                            r.extend(local_results);
                        }
                        Ok(())
                    })
                })
                .collect();

            for handle in handles {
                handle
                    .join()
                    .map_err(|_| WalError::IoError("Thread panicked".to_string()))??;
            }
        }

        let results = results
            .lock()
            .map(|r| r.clone())
            .map_err(|e| WalError::IoError(format!("Failed to acquire lock: {}", e)))?;

        Ok(self.merge_results(results))
    }

    /// Parse a single file
    fn parse_single_file(
        &self,
        path: &Path,
        recovery_mode: WalRecoveryMode,
        verify_checksum: bool,
    ) -> WalResult<RecoveryResult> {
        Self::parse_file_static(path, recovery_mode, verify_checksum)
    }

    /// Static method to parse a file (for parallel use)
    fn parse_file_static(
        path: &Path,
        recovery_mode: WalRecoveryMode,
        verify_checksum: bool,
    ) -> WalResult<RecoveryResult> {
        use std::io::Read;

        let mut result = RecoveryResult::default();

        let metadata = std::fs::metadata(path).map_err(|e| WalError::IoError(e.to_string()))?;

        if metadata.len() == 0 {
            return Ok(result);
        }

        let mut file = File::open(path).map_err(|e| WalError::IoError(e.to_string()))?;

        let file_size = metadata.len() as usize;
        let mut buffer = Vec::with_capacity(file_size);
        file.read_to_end(&mut buffer)
            .map_err(|e| WalError::IoError(e.to_string()))?;

        if buffer.len() < WAL_FILE_HEADER_SIZE {
            return Err(WalError::InvalidFileHeader);
        }

        let file_header = WalFileHeader::from_bytes(&buffer[..WAL_FILE_HEADER_SIZE])
            .ok_or(WalError::InvalidFileHeader)?;

        if !file_header.is_valid() {
            return Err(WalError::InvalidFileHeader);
        }

        let file_start_lsn = file_header.start_lsn();
        let mut fragment_buffer = FragmentBuffer::new();

        let mut offset = WAL_FILE_HEADER_SIZE;
        while offset + WAL_HEADER_SIZE <= buffer.len() {
            let header = match WalHeader::from_bytes(&buffer[offset..offset + WAL_HEADER_SIZE]) {
                Some(h) => h,
                None => {
                    result.corrupted_count += 1;
                    offset += 1;
                    continue;
                }
            };

            if header.timestamp == 0 && header.length == 0 && header.lsn == 0 {
                break;
            }

            let payload_start = offset + WAL_HEADER_SIZE;
            let payload_end = payload_start + header.length as usize;

            if payload_end > buffer.len() {
                match recovery_mode {
                    WalRecoveryMode::AbortOnCorruption => {
                        return Err(WalError::Corrupted(format!(
                            "Truncated entry at offset {}",
                            offset
                        )));
                    }
                    _ => {
                        result.corrupted_count += 1;
                        break;
                    }
                }
            }

            let payload = buffer[payload_start..payload_end].to_vec();

            if verify_checksum && header.checksum != 0 {
                let computed = Self::compute_checksum_static(&header, &payload);
                if computed != header.checksum {
                    match recovery_mode {
                        WalRecoveryMode::AbortOnCorruption => {
                            return Err(WalError::ChecksumMismatch {
                                expected: header.checksum,
                                actual: computed,
                            });
                        }
                        _ => {
                            result.corrupted_count += 1;
                            offset = payload_end;
                            continue;
                        }
                    }
                }
            }

            let final_payload = if header.is_compressed() {
                match Self::decompress_payload_static(&payload, header.compression()) {
                    Ok(decompressed) => decompressed,
                    Err(e) => match recovery_mode {
                        WalRecoveryMode::AbortOnCorruption => {
                            return Err(e);
                        }
                        _ => {
                            result.corrupted_count += 1;
                            offset = payload_end;
                            continue;
                        }
                    },
                }
            } else {
                payload
            };

            let record_type = header.record_type;
            let entry_lsn = header.lsn();

            if record_type == RecordType::Full {
                result.all_entries.push(ParsedWalEntry {
                    header,
                    payload: final_payload,
                    checksum_valid: true,
                    offset,
                    lsn: entry_lsn,
                    prev_lsn: header.prev_lsn(),
                    file_start_lsn,
                });

                result.last_timestamp = result.last_timestamp.max(header.timestamp);
                if entry_lsn > result.last_lsn {
                    result.last_lsn = entry_lsn;
                }
            } else {
                let is_complete = fragment_buffer.add_fragment(header, final_payload);

                if is_complete {
                    let assembled = fragment_buffer.assemble().unwrap_or_default();
                    let first_header = fragment_buffer
                        .get_first_header()
                        .cloned()
                        .unwrap_or(header);
                    fragment_buffer.reset();

                    let first_entry_lsn = first_header.lsn();

                    result.all_entries.push(ParsedWalEntry {
                        header: first_header,
                        payload: assembled,
                        checksum_valid: true,
                        offset,
                        lsn: first_entry_lsn,
                        prev_lsn: first_header.prev_lsn(),
                        file_start_lsn,
                    });

                    result.last_timestamp = result.last_timestamp.max(first_header.timestamp);
                    if first_entry_lsn > result.last_lsn {
                        result.last_lsn = first_entry_lsn;
                    }
                }
            }

            offset = payload_end;
        }

        Ok(result)
    }

    /// Compute checksum for verification (static version)
    fn compute_checksum_static(header: &WalHeader, payload: &[u8]) -> u32 {
        use crc32fast::Hasher;
        let mut hasher = Hasher::new();
        hasher.update(&header.length.to_le_bytes());
        hasher.update(&[
            header.op_type,
            header.is_update as u8,
            header.record_type as u8,
        ]);
        hasher.update(&header.flags.to_le_bytes());
        hasher.update(&header.timestamp.to_le_bytes());
        hasher.update(&header.lsn.to_le_bytes());
        hasher.update(&header.prev_lsn.to_le_bytes());
        hasher.update(payload);
        hasher.finalize()
    }

    /// Decompress payload (static version)
    fn decompress_payload_static(
        payload: &[u8],
        compression: WalCompression,
    ) -> WalResult<Vec<u8>> {
        match compression {
            WalCompression::Zstd => {
                zstd::decode_all(payload).map_err(|e| WalError::DeserializationError(e.to_string()))
            }
            WalCompression::None => Ok(payload.to_vec()),
        }
    }

    /// Merge multiple recovery results into one
    fn merge_results(&self, results: Vec<RecoveryResult>) -> RecoveryResult {
        let mut merged = RecoveryResult::default();

        for result in results {
            if result.last_timestamp > merged.last_timestamp {
                merged.last_timestamp = result.last_timestamp;
            }
            if result.last_lsn > merged.last_lsn {
                merged.last_lsn = result.last_lsn;
            }
            merged.corrupted_count += result.corrupted_count;
            merged.skipped_count += result.skipped_count;
            merged.all_entries.extend(result.all_entries);
        }

        merged.all_entries.sort_by_key(|e| e.lsn);

        merged
    }
}

impl Default for ParallelWalParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse result for a single WAL entry
#[derive(Debug, Clone)]
pub struct ParsedWalEntry {
    pub header: WalHeader,
    pub payload: Vec<u8>,
    pub checksum_valid: bool,
    pub offset: usize,
    pub lsn: Lsn,
    pub prev_lsn: Lsn,
    pub file_start_lsn: Lsn,
}

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
    /// Opened files
    files: Vec<File>,
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
    /// Fragment buffer for reassembling large records
    fragment_buffer: FragmentBuffer,
}

/// Buffer for reassembling fragmented WAL records
#[derive(Default)]
struct FragmentBuffer {
    /// Current fragments being assembled
    fragments: Vec<Vec<u8>>,
    /// Header of the first fragment
    first_header: Option<WalHeader>,
    /// Expected next record type
    expected_next: Option<RecordType>,
}

impl FragmentBuffer {
    fn new() -> Self {
        Self {
            fragments: Vec::new(),
            first_header: None,
            expected_next: None,
        }
    }

    fn reset(&mut self) {
        self.fragments.clear();
        self.first_header = None;
        self.expected_next = None;
    }

    fn add_fragment(&mut self, header: WalHeader, payload: Vec<u8>) -> bool {
        let record_type = header.record_type;

        match record_type {
            RecordType::Full => {
                self.reset();
                true
            }
            RecordType::First => {
                self.reset();
                self.fragments.push(payload);
                self.first_header = Some(header);
                self.expected_next = Some(RecordType::Middle);
                false
            }
            RecordType::Middle => {
                if self.expected_next != Some(RecordType::Middle)
                    && self.expected_next != Some(RecordType::Last)
                {
                    self.reset();
                    return false;
                }
                self.fragments.push(payload);
                self.expected_next = Some(RecordType::Middle);
                false
            }
            RecordType::Last => {
                if self.expected_next != Some(RecordType::Middle)
                    && self.expected_next != Some(RecordType::Last)
                {
                    self.reset();
                    return false;
                }
                self.fragments.push(payload);
                true
            }
        }
    }

    fn assemble(&self) -> Option<Vec<u8>> {
        if self.fragments.is_empty() {
            return None;
        }
        let mut result = Vec::new();
        for fragment in &self.fragments {
            result.extend_from_slice(fragment);
        }
        Some(result)
    }

    fn get_first_header(&self) -> Option<&WalHeader> {
        self.first_header.as_ref()
    }
}

impl LocalWalParser {
    /// Create a new local WAL parser
    pub fn new() -> Self {
        Self {
            wal_dir: None,
            all_entries: Vec::new(),
            last_timestamp: 0,
            last_lsn: Lsn::ZERO,
            files: Vec::new(),
            file_headers: Vec::new(),
            recovery_mode: WalRecoveryMode::default(),
            verify_checksum: true,
            corrupted_count: 0,
            skipped_count: 0,
            fragment_buffer: FragmentBuffer::new(),
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
        if !wal_dir.exists() {
            if self.recovery_mode == WalRecoveryMode::ErrorIfMissing {
                return Err(WalError::FileNotFound(
                    wal_dir.to_string_lossy().to_string(),
                ));
            }
            std::fs::create_dir_all(wal_dir).map_err(|e| WalError::IoError(e.to_string()))?;
            return Ok(());
        }

        let mut wal_files: Vec<PathBuf> = std::fs::read_dir(wal_dir)
            .map_err(|e| WalError::IoError(e.to_string()))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "wal")
                    || path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("thread_") && n.contains("_wal_"))
            })
            .collect();

        wal_files.sort();

        if wal_files.is_empty() && self.recovery_mode == WalRecoveryMode::ErrorIfMissing {
            return Err(WalError::FileNotFound("No WAL files found".to_string()));
        }

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
        use std::io::Read;

        let metadata = std::fs::metadata(path).map_err(|e| WalError::IoError(e.to_string()))?;

        if metadata.len() == 0 {
            return Ok(());
        }

        let mut file = File::open(path).map_err(|e| WalError::IoError(e.to_string()))?;

        let file_size = metadata.len() as usize;
        let mut buffer = Vec::with_capacity(file_size);
        file.read_to_end(&mut buffer)
            .map_err(|e| WalError::IoError(e.to_string()))?;

        self.files.push(file);

        if buffer.len() < WAL_FILE_HEADER_SIZE {
            return Err(WalError::InvalidFileHeader);
        }

        let file_header = WalFileHeader::from_bytes(&buffer[..WAL_FILE_HEADER_SIZE])
            .ok_or(WalError::InvalidFileHeader)?;

        if !file_header.is_valid() {
            return Err(WalError::InvalidFileHeader);
        }

        let file_start_lsn = file_header.start_lsn();
        self.file_headers.push(file_header);

        let mut offset = WAL_FILE_HEADER_SIZE;
        while offset + WAL_HEADER_SIZE <= buffer.len() {
            let header = match WalHeader::from_bytes(&buffer[offset..offset + WAL_HEADER_SIZE]) {
                Some(h) => h,
                None => {
                    self.corrupted_count += 1;
                    offset += 1;
                    continue;
                }
            };

            if header.timestamp == 0 && header.length == 0 && header.lsn == 0 {
                break;
            }

            let payload_start = offset + WAL_HEADER_SIZE;
            let payload_end = payload_start + header.length as usize;

            if payload_end > buffer.len() {
                match self.recovery_mode {
                    WalRecoveryMode::AbortOnCorruption => {
                        return Err(WalError::Corrupted(format!(
                            "Truncated entry at offset {}",
                            offset
                        )));
                    }
                    _ => {
                        self.corrupted_count += 1;
                        break;
                    }
                }
            }

            let payload = buffer[payload_start..payload_end].to_vec();

            if self.verify_checksum && header.checksum != 0 && !header.verify_checksum(&payload) {
                match self.recovery_mode {
                    WalRecoveryMode::AbortOnCorruption => {
                        return Err(WalError::ChecksumMismatch {
                            expected: header.checksum,
                            actual: self.compute_checksum(&header, &payload),
                        });
                    }
                    _ => {
                        self.corrupted_count += 1;
                        offset = payload_end;
                        continue;
                    }
                }
            }

            let final_payload = if header.is_compressed() {
                match Self::decompress_payload(&payload, header.compression()) {
                    Ok(decompressed) => decompressed,
                    Err(e) => match self.recovery_mode {
                        WalRecoveryMode::AbortOnCorruption => {
                            return Err(e);
                        }
                        _ => {
                            self.corrupted_count += 1;
                            offset = payload_end;
                            continue;
                        }
                    },
                }
            } else {
                payload
            };

            let record_type = header.record_type;
            let entry_lsn = header.lsn();

            if record_type == RecordType::Full {
                self.all_entries.push(ParsedWalEntry {
                    header,
                    payload: final_payload,
                    checksum_valid: true,
                    offset,
                    lsn: entry_lsn,
                    prev_lsn: header.prev_lsn(),
                    file_start_lsn,
                });

                self.last_timestamp = self.last_timestamp.max(header.timestamp);
                if entry_lsn > self.last_lsn {
                    self.last_lsn = entry_lsn;
                }
            } else {
                let is_complete = self.fragment_buffer.add_fragment(header, final_payload);

                if is_complete {
                    let assembled = self.fragment_buffer.assemble().unwrap_or_default();
                    let first_header = self
                        .fragment_buffer
                        .get_first_header()
                        .cloned()
                        .unwrap_or(header);
                    self.fragment_buffer.reset();

                    let first_entry_lsn = first_header.lsn();

                    self.all_entries.push(ParsedWalEntry {
                        header: first_header,
                        payload: assembled,
                        checksum_valid: true,
                        offset,
                        lsn: first_entry_lsn,
                        prev_lsn: first_header.prev_lsn(),
                        file_start_lsn,
                    });

                    self.last_timestamp = self.last_timestamp.max(first_header.timestamp);
                    if first_entry_lsn > self.last_lsn {
                        self.last_lsn = first_entry_lsn;
                    }
                }
            }

            offset = payload_end;
        }

        Ok(())
    }

    /// Compute checksum for verification
    fn compute_checksum(&self, header: &WalHeader, payload: &[u8]) -> u32 {
        use crc32fast::Hasher;
        let mut hasher = Hasher::new();
        hasher.update(&header.length.to_le_bytes());
        hasher.update(&[
            header.op_type,
            header.is_update as u8,
            header.record_type as u8,
        ]);
        hasher.update(&header.flags.to_le_bytes());
        hasher.update(&header.timestamp.to_le_bytes());
        hasher.update(&header.lsn.to_le_bytes());
        hasher.update(&header.prev_lsn.to_le_bytes());
        hasher.update(payload);
        hasher.finalize()
    }

    /// Decompress payload
    fn decompress_payload(payload: &[u8], compression: WalCompression) -> WalResult<Vec<u8>> {
        match compression {
            WalCompression::Zstd => {
                zstd::decode_all(payload).map_err(|e| WalError::DeserializationError(e.to_string()))
            }
            WalCompression::None => Ok(payload.to_vec()),
        }
    }

    /// Get all WAL entries as an iterator
    pub fn iter_entries(&self) -> WalEntryIter<'_> {
        WalEntryIter::new(self)
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
        self.files.clear();
        self.file_headers.clear();
        self.last_timestamp = 0;
        self.last_lsn = Lsn::ZERO;
        self.corrupted_count = 0;
        self.skipped_count = 0;
    }

    fn last_timestamp(&self) -> Timestamp {
        self.last_timestamp
    }

    fn get_update_wals(&self) -> &[UpdateWalUnit] {
        &[]
    }
}

/// Iterator over WAL entries
pub struct WalEntryIter<'a> {
    entries: std::slice::Iter<'a, ParsedWalEntry>,
}

impl<'a> WalEntryIter<'a> {
    fn new(parser: &'a LocalWalParser) -> Self {
        Self {
            entries: parser.all_entries.iter(),
        }
    }
}

impl<'a> Iterator for WalEntryIter<'a> {
    type Item = &'a ParsedWalEntry;

    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next()
    }
}

/// WAL parser factory
pub struct WalParserFactory;

impl WalParserFactory {
    /// Create a WAL parser based on the URI scheme
    pub fn create_wal_parser(wal_uri: &str) -> WalResult<Box<dyn WalParser>> {
        let scheme = Self::get_scheme(wal_uri);

        match scheme.as_str() {
            "file" | "" => Ok(Box::new(LocalWalParser::new())),
            _ => Err(WalError::IoError(format!(
                "Unknown WAL parser scheme: {}",
                scheme
            ))),
        }
    }

    fn get_scheme(uri: &str) -> String {
        if let Some(pos) = uri.find("://") {
            uri[..pos].to_string()
        } else {
            "file".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::wal::types::WalConfig;
    use crate::transaction::wal::writer::{LocalWalWriter, WalWriter};
    use crate::transaction::wal::WalOpType;
    use tempfile::TempDir;

    #[test]
    fn test_wal_parser() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        {
            let config = WalConfig::new().with_checksum(true);
            let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
            writer.open().expect("Failed to open WAL");

            writer
                .append_entry(WalOpType::InsertVertex, 1, b"vertex1")
                .expect("Failed to append");
            writer
                .append_entry(WalOpType::InsertVertex, 2, b"vertex2")
                .expect("Failed to append");
            writer
                .append_entry(WalOpType::UpdateVertexProp, 3, b"update1")
                .expect("Failed to append");

            writer.sync().expect("Failed to sync");
        }

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("Failed to parse WAL");

        assert_eq!(parser.last_timestamp(), 3);
        assert_eq!(parser.corrupted_count(), 0);
        assert_eq!(parser.all_entries.len(), 3);

        assert!(!parser.file_headers().is_empty());
        assert!(parser.file_headers()[0].is_valid());

        parser.close();
    }

    #[test]
    fn test_wal_entry_iter() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        {
            let mut writer = LocalWalWriter::new(&wal_path, 0);
            writer.open().expect("Failed to open WAL");

            writer
                .append_entry(WalOpType::InsertVertex, 1, b"data1")
                .expect("Failed to append");
            writer
                .append_entry(WalOpType::UpdateVertexProp, 2, b"data2")
                .expect("Failed to append");

            writer.sync().expect("Failed to sync");
        }

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("Failed to parse WAL");

        let entries: Vec<_> = parser.iter_entries().collect();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_wal_parser_with_recovery_mode() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        {
            let mut writer = LocalWalWriter::new(&wal_path, 0);
            writer.open().expect("Failed to open WAL");
            writer
                .append_entry(WalOpType::InsertVertex, 1, b"data")
                .expect("Failed to append");
            writer.sync().expect("Failed to sync");
        }

        let mut parser = LocalWalParser::with_recovery_mode(WalRecoveryMode::SkipCorruption);
        parser.open(&wal_path).expect("Failed to parse WAL");
        assert_eq!(parser.last_timestamp(), 1);
    }

    #[test]
    fn test_wal_parser_checksum_verification() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        {
            let config = WalConfig::new().with_checksum(true);
            let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
            writer.open().expect("Failed to open WAL");
            writer
                .append_entry(WalOpType::InsertVertex, 1, b"test_payload")
                .expect("Failed to append");
            writer.sync().expect("Failed to sync");
        }

        let mut parser = LocalWalParser::new().with_verify_checksum(true);
        parser.open(&wal_path).expect("Failed to parse WAL");

        assert_eq!(parser.corrupted_count(), 0);
        assert_eq!(parser.all_entries.len(), 1);
    }

    #[test]
    fn test_wal_parser_error_if_missing() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let non_existent_path = temp_dir.path().join("non_existent");
        let wal_path = non_existent_path.to_string_lossy().to_string();

        let mut parser = LocalWalParser::with_recovery_mode(WalRecoveryMode::ErrorIfMissing);
        let result = parser.open(&wal_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_parallel_wal_parser() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_dir = temp_dir.path();

        {
            let config = WalConfig::new().with_checksum(true);
            let mut writer1 =
                LocalWalWriter::with_config(&wal_dir.to_string_lossy(), 0, config.clone());
            writer1.open().expect("Failed to open WAL1");
            writer1
                .append_entry(WalOpType::InsertVertex, 1, b"payload1")
                .expect("Failed to append");
            writer1.sync().expect("Failed to sync");
            writer1.close();

            let mut writer2 = LocalWalWriter::with_config(&wal_dir.to_string_lossy(), 1, config);
            writer2.open().expect("Failed to open WAL2");
            writer2
                .append_entry(WalOpType::InsertVertex, 2, b"payload2")
                .expect("Failed to append");
            writer2.sync().expect("Failed to sync");
            writer2.close();
        }

        let parser = ParallelWalParser::new()
            .with_threads(2)
            .with_verify_checksum(true);
        let result = parser.parse_parallel(wal_dir).expect("Failed to parse");

        assert_eq!(result.corrupted_count, 0);
        assert!(result.last_lsn > Lsn::ZERO);
        assert_eq!(result.all_entries.len(), 2);
    }
}
