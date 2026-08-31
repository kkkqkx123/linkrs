//! Local file-based WAL writer

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use super::compression::{create_compressor, Compressor};
use super::group_commit::GroupCommitCoordinator;
use super::sync::elapsed_since;
use crate::wal::parser::{LocalWalParser, WalParser};
use graphdb_core::types::Timestamp;
use graphdb_core::wal::traits::WalWriter;
use graphdb_core::wal::types::{
    Lsn, RecordType, WalCompression, WalConfig, WalError, WalFileHeader, WalOpType, WalResult,
    WalStats,
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
        }
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
        self.current_lsn.store(lsn.as_u64(), Ordering::SeqCst);
    }

    pub fn file_size(&self) -> usize {
        self.file_size
    }

    pub fn file_used(&self) -> usize {
        self.file_used
    }

    pub fn get_stats(&self) -> &WalStats {
        &self.stats
    }

    pub fn reset_stats(&mut self) {
        self.stats = WalStats::new();
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
                self.current_lsn
                    .store(parser.last_lsn().as_u64(), Ordering::SeqCst);
                self.last_synced_lsn
                    .store(parser.last_lsn().as_u64(), Ordering::SeqCst);
            }
        }

        file.set_len(self.config.truncate_size as u64)?;

        self.file = Some(file);
        self.file_path = Some(path);
        self.file_size = self.config.truncate_size;
        self.file_used = 0;
        self.is_open.store(true, Ordering::SeqCst);

        self.write_file_header()?;

        Ok(())
    }

    fn close(&mut self) {
        if !self.is_open.swap(false, Ordering::SeqCst) {
            return;
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
        LocalWalWriter::append_entry(self, op_type, timestamp, payload)
    }

    fn sync(&self) -> WalResult<()> {
        self.check_poisoned()?;
        let current_lsn = self.current_lsn.load(Ordering::SeqCst);

        if let Some(ref coordinator) = self.group_commit {
            coordinator.record_appended(current_lsn);
            coordinator.append_and_wait(current_lsn)?;
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

    fn wait_for_durable(&self, appended_lsn: u64) -> WalResult<()> {
        if let Some(ref coordinator) = self.group_commit {
            coordinator.record_appended(appended_lsn);
            coordinator.append_and_wait(appended_lsn)
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
    use graphdb_core::wal::{
        EntityRef, IndexMutation, IndexOperation, OutboxIntent, WAL_SYNC_WIRE_VERSION,
    };
    use graphdb_core::wal::types::{ArchiveMode, WalHeader, WAL_FILE_HEADER_SIZE, WAL_MAX_RECORD_SIZE};
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
                entity_ref: EntityRef::Vertex(VertexId::from_int64(1)),
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
}
