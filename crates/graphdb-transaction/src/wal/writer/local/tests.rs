//! Local WAL writer tests: append paths, sync policies, rotation,
//! poison handling and async buffering.

use super::*;
use crate::wal::{
    collect_committed_transactions, LocalWalParser, SyncPolicy, TransactionWalEntry, WalParser,
};
use graphdb_core::types::{
    IdempotencyKey, IndexGeneration, OrderingKey, TargetId, TransactionId, VertexId,
};
use graphdb_core::wal::types::{ArchiveMode, WalHeader, WAL_FILE_HEADER_SIZE, WAL_MAX_RECORD_SIZE};
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
