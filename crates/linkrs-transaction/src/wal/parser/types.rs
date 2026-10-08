//! Parser result types: recovery result, parsed entry and entry iterator.

use linkrs_core::types::Timestamp;
use linkrs_core::wal::types::{Lsn, WalHeader};

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

/// Iterator over WAL entries
pub struct WalEntryIter<'a> {
    entries: std::slice::Iter<'a, ParsedWalEntry>,
}

impl<'a> WalEntryIter<'a> {
    pub(super) fn new(entries: &'a [ParsedWalEntry]) -> Self {
        Self {
            entries: entries.iter(),
        }
    }
}

impl<'a> Iterator for WalEntryIter<'a> {
    type Item = &'a ParsedWalEntry;

    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next()
    }
}
