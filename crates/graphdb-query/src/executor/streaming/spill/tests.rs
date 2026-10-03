    use graphdb_core::value::NullType;

    fn sample_rows(n: usize) -> Vec<Vec<Value>> {
        (0..n)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::string(format!("val_{}", i)),
                    Value::Null(NullType::Null),
                    Value::Bool(i % 2 == 0),
                ]
            })
            .collect()
    }

    // ── Run writer / reader tests ────────────────────────────────────────

    #[test]
    fn test_run_writer_reader_roundtrip() {
        let manager = SpillManager::new(SpillConfig::default(), 201).unwrap();
        let fp: u64 = 0x123456789abcdef0;
        let mut writer = manager.create_run_writer(fp).unwrap();
        let rows = sample_rows(100);
        writer.write_rows(&rows).unwrap();
        let run = writer.finalize().unwrap();
        assert_eq!(run.row_count, 100);
        assert_eq!(run.schema_fingerprint, fp);

        let mut reader = RunReader::open(&run).unwrap();
        assert_eq!(reader.read_all().unwrap(), rows);
    }

    #[test]
    fn test_run_empty_file() {
        let manager = SpillManager::new(SpillConfig::default(), 202).unwrap();
        let writer = manager.create_run_writer(0).unwrap();
        let run = writer.finalize().unwrap();
        assert_eq!(run.row_count, 0);

        let mut reader = RunReader::open(&run).unwrap();
        assert!(reader.read_row().unwrap().is_none());
    }

    #[test]
    fn test_run_schema_fingerprint_mismatch() {
        let manager = SpillManager::new(SpillConfig::default(), 203).unwrap();
        let mut writer = manager.create_run_writer(42).unwrap();
        writer.write_rows(&sample_rows(5)).unwrap();
        let run = writer.finalize().unwrap();
        assert_eq!(run.schema_fingerprint, 42);

        // Open with wrong fingerprint
        let wrong_meta = SpilledRun {
            path: run.path.clone(),
            row_count: 0,
            byte_size: 0,
            schema_fingerprint: 99,
        };
        let result = RunReader::open(&wrong_meta);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("schema fingerprint mismatch"));
    }

    #[test]
    fn test_run_checksum_corruption_detected() {
        let manager = SpillManager::new(SpillConfig::default(), 204).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();
        writer.write_rows(&sample_rows(10)).unwrap();
        let run = writer.finalize().unwrap();

        // Corrupt the file by truncating it. The header stays valid, so the
        // corruption surfaces when the section frame is decoded.
        let file_size = std::fs::metadata(&run.path).unwrap().len();
        let corrupted_file = std::fs::File::options()
            .write(true)
            .open(&run.path)
            .unwrap();
        corrupted_file.set_len(file_size - 4).unwrap(); // truncate last 4 bytes
        drop(corrupted_file);

        let mut reader = RunReader::open(&run).unwrap();
        let err = reader.read_all().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("checksum mismatch") || msg.contains("spill run:"));
    }

    #[test]
    fn test_run_invalid_magic() {
        let manager = SpillManager::new(SpillConfig::default(), 205).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();
        writer.write_rows(&sample_rows(5)).unwrap();
        let run = writer.finalize().unwrap();

        // Overwrite magic bytes with garbage
        let f = std::fs::File::options()
            .write(true)
            .open(&run.path)
            .unwrap();
        f.set_len(4).unwrap(); // truncate to just 4 bytes of junk
        drop(f);

        let result = RunReader::open(&run);
        assert!(result.is_err());
    }

    #[test]
    fn test_disk_quota_exceeded() {
        let quota = DiskQuota::new(100);
        quota.try_reserve(50).unwrap();
        assert_eq!(quota.current(), 50);
        quota.try_reserve(30).unwrap();
        assert_eq!(quota.current(), 80);
        // This should exceed
        let result = quota.try_reserve(30);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Disk quota exceeded"));
    }

    #[test]
    fn test_disk_quota_release() {
        let quota = DiskQuota::new(100);
        quota.try_reserve(80).unwrap();
        assert_eq!(quota.current(), 80);
        quota.release(30);
        assert_eq!(quota.current(), 50);
        // After releasing, we can reserve up to the remaining capacity
        quota.try_reserve(50).unwrap();
        assert_eq!(quota.current(), 100);
    }

    #[test]
    fn test_disk_quota_unlimited() {
        let quota = DiskQuota::new(0); // 0 = unlimited
        quota.try_reserve(u64::MAX).unwrap();
        assert!(quota.current() > 0);
    }

    #[test]
    fn test_run_compression_roundtrip() {
        let manager = SpillManager::new(SpillConfig::default(), 206).unwrap();
        let fp: u64 = 0xabcdef;
        let mut writer = manager.create_run_writer(fp).unwrap();

        let rows = sample_rows(200);
        writer.write_rows(&rows).unwrap();
        let run = writer.finalize().unwrap();
        assert_eq!(run.row_count, 200);

        let mut reader = RunReader::open(&run).unwrap();
        assert_eq!(reader.read_all().unwrap(), rows);
        assert_eq!(reader.header().version, 3);
        assert!(reader.header().section_count >= 1);
    }

    #[test]
    fn test_run_small_data_uncompressed() {
        let manager = SpillManager::new(SpillConfig::default(), 207).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();

        let rows = sample_rows(2);
        writer.write_rows(&rows).unwrap();
        let run = writer.finalize().unwrap();

        let mut reader = RunReader::open(&run).unwrap();
        assert_eq!(reader.read_all().unwrap(), rows);
        assert_eq!(reader.header().section_count, 1);
        assert_eq!(reader.header().num_columns, 4);
    }

    #[test]
    fn test_run_multi_section_roundtrip() {
        let manager = SpillManager::new(SpillConfig::default(), 213).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();
        let rows = sample_rows(2500);
        writer.write_rows(&rows).unwrap();
        let run = writer.finalize().unwrap();
        assert_eq!(run.row_count, 2500);

        let mut reader = RunReader::open(&run).unwrap();
        assert_eq!(reader.header().section_count, 3);
        // Streamed reads cross section boundaries transparently.
        assert_eq!(reader.read_all().unwrap(), rows);
    }

    #[test]
    fn test_run_sectioned_batch_reads() {
        let manager = SpillManager::new(SpillConfig::default(), 214).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();
        let rows = sample_rows(1500);
        writer
            .write_batch(&MaterializedBatch::from_rows(rows.clone()))
            .unwrap();
        let run = writer.finalize().unwrap();

        let mut reader = RunReader::open(&run).unwrap();
        let first = reader.read_batch(1000).unwrap().expect("first batch");
        assert_eq!(first.num_rows(), 1000);
        let rest = reader.read_all().unwrap();
        assert_eq!(rest.len(), 500);
        let mut combined = first.to_rows();
        combined.extend(rest);
        assert_eq!(combined, rows);
    }

    #[test]
    fn test_run_rejects_legacy_magic() {
        let manager = SpillManager::new(SpillConfig::default(), 215).unwrap();
        let writer = manager.create_run_writer(0).unwrap();
        let path = writer.path().to_path_buf();
        let run = writer.finalize().unwrap();
        // Overwrite with the legacy row-major magic.
        let mut buf = std::fs::read(&run.path).unwrap();
        buf[0..4].copy_from_slice(&[0x47, 0x52, 0x53, 0x50]);
        std::fs::write(&path, &buf).unwrap();
        let err = RunReader::open(&run).unwrap_err();
        assert!(err.to_string().contains("invalid magic"));
    }

    #[test]
    fn test_run_header_checksum_detects_corruption() {
        let manager = SpillManager::new(SpillConfig::default(), 216).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();
        writer.write_rows(&sample_rows(10)).unwrap();
        let run = writer.finalize().unwrap();
        let mut buf = std::fs::read(&run.path).unwrap();
        buf[16] ^= 0xff;
        std::fs::write(&run.path, &buf).unwrap();
        let err = RunReader::open(&run).unwrap_err();
        assert!(err.to_string().contains("header checksum mismatch"));
    }

    #[test]
    fn test_run_row_arity_mismatch_rejected() {
        let manager = SpillManager::new(SpillConfig::default(), 217).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();
        writer.write_rows(&sample_rows(3)).unwrap();
        let err = writer.write_row(&[Value::BigInt(1)]).unwrap_err();
        assert!(err.to_string().contains("row arity"));
    }

    #[test]
    fn test_finalize_run_enforces_quota_and_removes_file() {
        let quota = DiskQuota::new(1);
        let manager = SpillManager::new_with_quota(SpillConfig::default(), 208, quota).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();
        writer.write_rows(&sample_rows(10)).unwrap();
        let path = writer.path().to_path_buf();
        let err = manager.finalize_run(writer).unwrap_err();
        assert!(err.to_string().contains("Disk quota exceeded"));
        assert!(
            !path.exists(),
            "quota failure must not leave an orphaned run file"
        );
        assert_eq!(manager.spilled_bytes(), 0);
    }

    #[test]
    fn test_finalize_run_tracks_bytes() {
        let manager = SpillManager::new(SpillConfig::default(), 209).unwrap();
        let mut writer = manager.create_run_writer(0).unwrap();
        writer.write_rows(&sample_rows(10)).unwrap();
        let run = manager.finalize_run(writer).unwrap();
        assert_eq!(manager.spilled_bytes(), run.byte_size);
    }

    #[test]
    fn test_create_run_writer_enforces_max_spill_files() {
        let config = SpillConfig {
            temp_dir: None,
            max_spill_files: 2,
            collect_spill_rows: None,
        };
        let manager = SpillManager::new(config, 210).unwrap();
        let _first = manager.create_run_writer(0).unwrap();
        let _second = manager.create_run_writer(0).unwrap();
        let err = manager.create_run_writer(0).unwrap_err();
        assert!(err.to_string().contains("too many spill files"));
    }

    #[test]
    fn test_collect_spill_threshold_defaults_and_disables() {
        let manager = SpillManager::new(SpillConfig::default(), 211).unwrap();
        assert_eq!(
            manager.collector_spill_threshold(),
            COLLECTOR_SPILL_ROWS_DEFAULT
        );
        let disabled = SpillManager::new(
            SpillConfig {
                temp_dir: None,
                max_spill_files: 64,
                collect_spill_rows: Some(0),
            },
            212,
        )
        .unwrap();
        assert_eq!(disabled.collector_spill_threshold(), u64::MAX);
    }

    #[test]
    fn test_write_batch_read_batch_roundtrip() {
        use graphdb_core::columnar::MaterializedBatch;
        let manager = SpillManager::new(SpillConfig::default(), 213).unwrap();
        let mut writer = manager.create_run_writer(7).unwrap();
        let batch = MaterializedBatch::from_rows(sample_rows(50));
        writer.write_batch(&batch).unwrap();
        let run = writer.finalize().unwrap();
        assert_eq!(run.row_count, 50);

        let mut reader = RunReader::open(&run).unwrap();
        let first = reader.read_batch(20).unwrap().expect("first slice");
        assert_eq!(first.num_rows(), 20);
        let rest = reader.read_batch(100).unwrap().expect("rest slice");
        assert_eq!(rest.num_rows(), 30);
        assert!(reader.read_batch(10).unwrap().is_none());
        let mut combined = first.to_rows();
        combined.extend(rest.to_rows());
        assert_eq!(combined, sample_rows(50));
    }

    #[test]
    fn test_hash_column_partition_keys_only() {
        use graphdb_core::columnar::MaterializedBatch;
        let batch = MaterializedBatch::from_rows(vec![
            vec![Value::BigInt(1), Value::string("same")],
            vec![Value::BigInt(2), Value::string("same")],
            vec![Value::BigInt(1), Value::string("same")],
        ]);
        // Full-row hash separates row 0 and row 1.
        let full = hash_column_partition(&batch, &[], 1024);
        assert_ne!(full[0], full[1]);
        // Key column [1] is identical: all partitions equal.
        let keyed = hash_column_partition(&batch, &[1], 1024);
        assert_eq!(keyed[0], keyed[1]);
        assert_eq!(keyed[0], keyed[2]);
        // Key column [0]: rows 0 and 2 share a partition.
        let keyed0 = hash_column_partition(&batch, &[0], 1024);
        assert_eq!(keyed0[0], keyed0[2]);
    }
