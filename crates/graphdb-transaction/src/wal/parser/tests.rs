//! Parser tests: sequential and parallel parsing, corruption handling.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};

use tempfile::TempDir;

use super::{LocalWalParser, ParallelWalParser, WalParser};
use crate::wal::writer::{LocalWalWriter, WalWriter};
use crate::wal::WalOpType;
use graphdb_core::wal::types::{
    Lsn, WalConfig, WalRecoveryMode, WAL_FILE_HEADER_SIZE, WAL_HEADER_SIZE,
};

fn wal_file(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::read_dir(dir)
        .expect("WAL directory should be readable")
        .map(|entry| {
            entry
                .expect("WAL directory entry should be readable")
                .path()
        })
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("thread_") && name.contains("_wal_"))
        })
        .expect("WAL file should exist")
}

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
    assert_eq!(parser.iter_entries().count(), 3);

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
    assert_eq!(parser.iter_entries().count(), 1);
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
fn default_recovery_mode_rejects_checksum_corruption() {
    let temp_dir = TempDir::new().expect("temporary directory should be created");
    let wal_path = temp_dir.path().to_string_lossy().to_string();

    let mut writer =
        LocalWalWriter::with_config(&wal_path, 0, WalConfig::new().with_checksum(true));
    writer.open().expect("WAL should open");
    writer
        .append_entry(WalOpType::InsertVertex, 1, b"payload")
        .expect("WAL entry should append");
    writer.sync().expect("WAL should sync");
    writer.close();

    let path = wal_file(temp_dir.path());
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .expect("WAL file should open");
    file.seek(SeekFrom::Start(
        (WAL_FILE_HEADER_SIZE + WAL_HEADER_SIZE) as u64,
    ))
    .expect("payload offset should be valid");
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).expect("payload should exist");
    byte[0] ^= 0xFF;
    file.seek(SeekFrom::Start(
        (WAL_FILE_HEADER_SIZE + WAL_HEADER_SIZE) as u64,
    ))
    .expect("payload offset should be valid");
    file.write_all(&byte).expect("payload should be corrupted");
    file.sync_all().expect("corruption should be durable");

    let mut parser = LocalWalParser::new();
    assert!(parser.open(&wal_path).is_err());
}

#[test]
fn default_recovery_mode_rejects_unknown_operation_type() {
    let temp_dir = TempDir::new().expect("temporary directory should be created");
    let wal_path = temp_dir.path().to_string_lossy().to_string();

    let mut writer =
        LocalWalWriter::with_config(&wal_path, 0, WalConfig::new().with_checksum(false));
    writer.open().expect("WAL should open");
    writer
        .append_entry(WalOpType::InsertVertex, 1, b"payload")
        .expect("WAL entry should append");
    writer.sync().expect("WAL should sync");
    writer.close();

    let path = wal_file(temp_dir.path());
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .expect("WAL file should open");
    file.seek(SeekFrom::Start((WAL_FILE_HEADER_SIZE + 4) as u64))
        .expect("operation type offset should be valid");
    file.write_all(&[255])
        .expect("operation type should be corrupted");
    file.sync_all().expect("corruption should be durable");

    let mut parser = LocalWalParser::new();
    assert!(parser.open(&wal_path).is_err());
}

#[test]
fn torn_tail_is_reported_but_prior_entries_remain_parseable() {
    let temp_dir = TempDir::new().expect("temporary directory should be created");
    let wal_path = temp_dir.path().to_string_lossy().to_string();

    let mut writer =
        LocalWalWriter::with_config(&wal_path, 0, WalConfig::new().with_checksum(true));
    writer.open().expect("WAL should open");
    writer
        .append_entry(WalOpType::InsertVertex, 1, b"first")
        .expect("first WAL entry should append");
    writer
        .append_entry(WalOpType::InsertVertex, 2, b"second")
        .expect("second WAL entry should append");
    let used = writer.file_used();
    writer.sync().expect("WAL should sync");
    writer.close();

    let path = wal_file(temp_dir.path());
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("WAL file should open");
    file.set_len((used - 1) as u64)
        .expect("WAL tail should be truncated");

    let mut parser = LocalWalParser::new();
    parser
        .open(&wal_path)
        .expect("torn tail should be recoverable");
    assert_eq!(parser.iter_entries().count(), 1);
    assert_eq!(parser.corrupted_count(), 1);
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
