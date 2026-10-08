//! Parallel WAL parser: thread-sharded scan over discovered files with
//! merge-sorted recovery results.

use std::path::Path;

use linkrs_core::wal::types::{WalRecoveryMode, WalResult};

use super::discover::{self, read_wal_file};
use super::scan::parse_wal_file_bytes;
use super::types::RecoveryResult;

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
        let wal_files = discover::discover_wal_files(wal_dir, self.recovery_mode)?;

        let recovery_mode = self.recovery_mode;
        let verify_checksum = self.verify_checksum;

        if wal_files.len() <= self.num_threads {
            let mut results = Vec::with_capacity(wal_files.len());
            for path in wal_files {
                results.push(Self::parse_file(&path, recovery_mode, verify_checksum)?);
            }
            return Ok(self.merge_results(results));
        }

        let chunk_size = wal_files.len().div_ceil(self.num_threads);
        let chunks: Vec<Vec<std::path::PathBuf>> =
            wal_files.chunks(chunk_size).map(|c| c.to_vec()).collect();

        let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(chunks.len())));

        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let results = std::sync::Arc::clone(&results);

                std::thread::spawn(move || {
                    let mut local_results = Vec::new();
                    for path in chunk {
                        match Self::parse_file(&path, recovery_mode, verify_checksum) {
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
            handle.join().map_err(|_| {
                linkrs_core::wal::types::WalError::IoError("Thread panicked".to_string())
            })??;
        }

        let results = results.lock().map(|r| r.clone()).map_err(|e| {
            linkrs_core::wal::types::WalError::IoError(format!("Failed to acquire lock: {}", e))
        })?;

        Ok(self.merge_results(results))
    }

    /// Parse a single WAL file with shared discovery and byte scan.
    fn parse_file(
        path: &Path,
        recovery_mode: WalRecoveryMode,
        verify_checksum: bool,
    ) -> WalResult<RecoveryResult> {
        let Some(data) = read_wal_file(path, verify_checksum)? else {
            return Ok(RecoveryResult::default());
        };
        parse_wal_file_bytes(
            &data.buffer,
            data.file_header.start_lsn(),
            recovery_mode,
            verify_checksum,
        )
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
