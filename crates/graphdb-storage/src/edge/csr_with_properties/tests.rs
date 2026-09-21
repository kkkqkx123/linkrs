use super::*;
use crate::encoding::EncodingType;
use graphdb_core::DataType;

fn schema() -> Vec<PropertySchema> {
    vec![
        PropertySchema::new("weight".to_string(), 0, DataType::Double),
        PropertySchema::new("label".to_string(), 1, DataType::String).nullable(true),
    ]
}

#[test]
fn edge_id_access() {
    let mut csr = CsrWithProperties::new(schema());
    let eid0 = EdgeId(1);
    let eid1 = EdgeId(2);
    csr.insert_for_edge(eid0, &[("weight".to_string(), Value::Double(1.5))], 10)
        .unwrap();
    csr.insert_for_edge(eid1, &[("weight".to_string(), Value::Double(2.5))], 10)
        .unwrap();
    let p0 = csr.get_by_edge_id(eid0, 10).unwrap();
    assert!(p0
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(1.5))));
    let by_id = csr.get_by_edge_id(eid1, 10).unwrap();
    assert!(by_id
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.5))));
    assert_eq!(csr.row_count(), 2);
}

#[test]
fn positioned_insert_matches_named_insert() {
    let mut csr = CsrWithProperties::new(schema());
    let eid = EdgeId(11);
    // Column positions follow schema order: weight=0, label=1.
    csr.insert_for_edge_at(eid, &[(0, Value::Double(4.5))], 10)
        .unwrap();
    let got = csr.get_by_edge_id(eid, 10).unwrap();
    assert!(got
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(4.5))));

    // Out-of-range positions fail instead of writing the wrong column.
    assert!(csr
        .insert_for_edge_at(EdgeId(12), &[(9, Value::Double(1.0))], 10)
        .is_err());

    // Unknown names on the named path resolve to no column, so the
    // non-nullable column keeps its missing default and fails loudly.
    assert!(csr
        .insert_for_edge(EdgeId(13), &[("nope".to_string(), Value::Double(1.0))], 10)
        .is_err());
}

#[test]
fn visibility() {
    let mut csr = CsrWithProperties::new(schema());
    let eid = EdgeId(99);
    csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(3.0))], 100)
        .unwrap();
    assert!(csr.get_by_edge_id(eid, 99).is_none());
    assert!(csr.get_by_edge_id(eid, 100).is_some());
    csr.mark_deleted(eid, 150);
    assert!(csr.get_by_edge_id(eid, 149).is_some());
    assert!(csr.get_by_edge_id(eid, 150).is_none());
}

#[test]
fn columnar_repeatable_read() {
    let mut csr = CsrWithProperties::new(schema());
    let eid = EdgeId(42);
    csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
        .unwrap();
    // Snapshot read at 150 still observes the before-image (RepeatableRead).
    let old = csr.get_by_edge_id(eid, 150).unwrap();
    assert!(old
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(1.0))));
    // Newer readers observe the latest write.
    let got = csr.get_by_edge_id(eid, 250).unwrap();
    assert!(got
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
}

#[test]
fn columnar_property_version_gc_keeps_visible_snapshots() {
    let mut csr = CsrWithProperties::new(schema());
    let eid = EdgeId(7);
    csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
        .unwrap();
    csr.set_property_for_edge(eid, "weight", Some(Value::Double(3.0)), 300)
        .unwrap();
    // Entries ending at or before the cutoff are eligible; the chain
    // covering ts=200 must survive a GC at 150.
    assert_eq!(csr.gc_property_versions(150), 0);
    let at_200 = csr.get_by_edge_id(eid, 200).unwrap();
    assert!(at_200
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
    // Past every active snapshot, history is reclaimed but the latest
    // value stays readable.
    assert!(csr.gc_property_versions(300) >= 1);
    let at_300 = csr.get_by_edge_id(eid, 300).unwrap();
    assert!(at_300
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(3.0))));
}

#[test]
fn projected_read_returns_only_requested_columns() {
    let mut csr = CsrWithProperties::new(schema());
    let eid = EdgeId(5);
    csr.insert_for_edge(
        eid,
        &[
            ("weight".to_string(), Value::Double(1.5)),
            ("label".to_string(), Value::String("a".into())),
        ],
        100,
    )
    .unwrap();

    let all = csr.get_projected_by_edge_id(eid, 100, None).unwrap();
    assert_eq!(all.len(), 2);

    let subset = csr
        .get_projected_by_edge_id(eid, 100, Some(&["label".to_string()]))
        .unwrap();
    assert_eq!(
        subset,
        vec![("label".to_string(), Some(Value::String("a".into())))]
    );

    let topology_only = csr.get_projected_by_edge_id(eid, 100, Some(&[])).unwrap();
    assert!(topology_only.is_empty());

    let unknown = csr
        .get_projected_by_edge_id(eid, 100, Some(&["missing".to_string()]))
        .unwrap();
    assert!(unknown.is_empty());

    assert!(csr.get_projected_by_edge_id(eid, 99, None).is_none());
    assert!(csr
        .get_projected_by_edge_id(eid, 99, Some(&["weight".to_string()]))
        .is_none());
}

#[test]
fn dump_load_roundtrip() {
    let mut csr = CsrWithProperties::new(schema());
    let eid0 = EdgeId(10);
    let eid1 = EdgeId(11);

    csr.insert_for_edge(eid0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    csr.insert_for_edge(eid1, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    csr.mark_deleted(eid0, 150);

    let bytes = csr.dump();
    let mut loaded = CsrWithProperties::new(schema());
    loaded.load(&bytes).unwrap();

    assert_eq!(loaded.row_count(), 2);
    assert!(loaded
        .get_by_edge_id(eid1, 100)
        .unwrap()
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
    assert!(loaded.get_by_edge_id(eid0, 149).is_some());
    assert!(loaded.get_by_edge_id(eid0, 150).is_none());
}

#[test]
fn truncated_payload_is_rejected() {
    let mut csr = CsrWithProperties::new(schema());
    let eid = EdgeId(10);
    csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let bytes = csr.dump();
    let mut loaded = CsrWithProperties::new(schema());
    assert!(loaded.load(&bytes[..bytes.len() / 2]).is_err());
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut csr = CsrWithProperties::new(schema());
    csr.insert_for_edge(
        EdgeId(10),
        &[("weight".to_string(), Value::Double(1.0))],
        100,
    )
    .unwrap();
    let mut bytes = csr.dump();
    bytes.push(0xff);
    let mut loaded = CsrWithProperties::new(schema());
    assert!(loaded.load(&bytes).is_err());
}

fn typed_store() -> CsrWithProperties {
    CsrWithProperties::new(vec![
        PropertySchema::new("count".to_string(), 0, DataType::Int),
        PropertySchema::new("flag".to_string(), 1, DataType::Bool),
        PropertySchema::new("tag".to_string(), 2, DataType::String).nullable(true),
    ])
}

fn fill_typed_store(rows: i64) -> CsrWithProperties {
    let mut csr = typed_store();
    for i in 0..rows {
        csr.insert_for_edge(
            EdgeId(i as u64),
            &[
                ("count".to_string(), Value::Int(i as i32)),
                ("flag".to_string(), Value::Bool(i % 2 == 0)),
                (
                    "tag".to_string(),
                    Value::String(format!("tag{}", i % 4).into()),
                ),
            ],
            100,
        )
        .expect("typed insert should succeed");
    }
    csr
}

#[test]
fn dump_collapses_version_history_to_latest() {
    let mut csr = CsrWithProperties::new(schema());
    let eid = EdgeId(42);
    csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
        .unwrap();
    let bytes = csr.dump();
    let mut loaded = CsrWithProperties::new(schema());
    loaded.load(&bytes).unwrap();
    let collapsed = loaded
        .get_by_edge_id(eid, 150)
        .expect("row survives reload");
    assert!(collapsed
        .iter()
        .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
}

#[test]
fn dump_restores_recorded_encodings_with_current_values() {
    let mut csr = fill_typed_store(20);
    assert!(csr.auto_encode_properties() > 0);
    let before = csr.column_encoding_type("count");
    assert!(before.is_some_and(|enc| enc != EncodingType::None));
    let bytes = csr.dump();
    let mut loaded = typed_store();
    loaded.load(&bytes).unwrap();
    assert_eq!(loaded.column_encoding_type("count"), before);
    let got = loaded
        .get_by_edge_id(EdgeId(3), 200)
        .expect("row should read");
    assert!(got
        .iter()
        .any(|(k, v)| k == "count" && v == &Some(Value::Int(3))));
}

#[test]
fn dump_persists_statistics_without_refresh() {
    let mut csr = fill_typed_store(10);
    csr.refresh_column_stats();
    let before = csr
        .column_stats_snapshot("count")
        .expect("stats should exist");
    assert!(before.null_count.is_some());
    let bytes = csr.dump();
    let mut loaded = typed_store();
    loaded.load(&bytes).unwrap();
    let after = loaded
        .column_stats_snapshot("count")
        .expect("stats should survive reload");
    assert_eq!(after.row_count, before.row_count);
    assert_eq!(after.null_count, before.null_count);
    assert_eq!(after.min_value, before.min_value);
    assert_eq!(after.max_value, before.max_value);
}

#[test]
fn duplicate_column_identifiers_are_rejected() {
    let csr = fill_typed_store(2);
    let mut bytes = csr.dump();
    // Patch the second column header identifier to collide with the
    // first: name length plus name precedes the identifier.
    let mut cursor = 0usize;
    let read_u32 = |cursor: &mut usize| {
        let value = u32::from_le_bytes(bytes[*cursor..*cursor + 4].try_into().unwrap());
        *cursor += 4;
        value
    };
    let vis_len = read_u32(&mut cursor) as usize;
    cursor += vis_len * 9 + 4;
    let map_len = read_u32(&mut cursor) as usize;
    cursor += map_len * 12;
    let free_len = read_u32(&mut cursor) as usize;
    cursor += free_len * 4;
    let col_count = read_u32(&mut cursor);
    assert!(col_count >= 2);
    let first_name_len = read_u32(&mut cursor) as usize;
    cursor += first_name_len;
    let first_id = cursor;
    cursor += 4 + 1;
    let first_rows = read_u32(&mut cursor) as usize;
    // Cells hold typed values of unknown byte length here: skip them by
    // re-reading flags and lengths from the payload itself.
    for _ in 0..first_rows {
        let has = bytes[cursor];
        cursor += 1;
        if has == 1 {
            let vlen = read_u32(&mut cursor) as usize;
            cursor += vlen;
        }
    }
    let stats_flag = bytes[cursor];
    cursor += 1;
    if stats_flag == 1 {
        let stats_len = read_u32(&mut cursor) as usize;
        cursor += stats_len;
    }
    let second_name_len = read_u32(&mut cursor) as usize;
    cursor += second_name_len;
    let second_id = cursor;
    bytes.copy_within(first_id..first_id + 4, second_id);
    let mut loaded = typed_store();
    assert!(loaded.load(&bytes).is_err());
}

#[test]
fn auto_encode_selects_expected_schemes() {
    let mut csr = fill_typed_store(20);
    assert_eq!(csr.auto_encode_properties(), 3);
    assert_eq!(
        csr.column_encoding_type("count"),
        Some(EncodingType::BitPacking)
    );
    assert_eq!(csr.column_encoding_type("flag"), Some(EncodingType::Rle));
    assert_eq!(
        csr.column_encoding_type("tag"),
        Some(EncodingType::Dictionary)
    );
}

#[test]
fn auto_encode_preserves_values() {
    let mut csr = fill_typed_store(20);
    csr.auto_encode_properties();
    for i in 0..20 {
        let got = csr
            .get_by_edge_id(EdgeId(i as u64), 200)
            .expect("encoded row should stay readable");
        assert!(got
            .iter()
            .any(|(k, v)| k == "count" && v == &Some(Value::Int(i))));
        assert!(got
            .iter()
            .any(|(k, v)| k == "flag" && v == &Some(Value::Bool(i % 2 == 0))));
        let expected_tag = Value::String(format!("tag{}", i % 4).into());
        assert!(got
            .iter()
            .any(|(k, v)| k == "tag" && v == &Some(expected_tag.clone())));
    }
}

#[test]
fn auto_encode_constant_column_uses_single_value_storage() {
    let mut csr = CsrWithProperties::new(vec![PropertySchema::new(
        "level".to_string(),
        0,
        DataType::Int,
    )]);
    for i in 0..60 {
        csr.insert_for_edge(EdgeId(i), &[("level".to_string(), Value::Int(7))], 100)
            .expect("constant insert should succeed");
    }
    assert_eq!(csr.auto_encode_properties(), 1);
    assert_eq!(
        csr.column_encoding_type("level"),
        Some(EncodingType::Constant)
    );
    let got = csr.get_by_edge_id(EdgeId(3), 200).expect("row should read");
    assert!(got
        .iter()
        .any(|(k, v)| k == "level" && v == &Some(Value::Int(7))));
}

#[test]
fn encode_rejects_unknown_column_and_skips_empty() {
    let mut csr = typed_store();
    assert_eq!(csr.auto_encode_properties(), 0);
    assert!(csr
        .apply_encoding_to_column("missing", EncodingType::Rle, 255)
        .is_err());
    assert_eq!(csr.column_encoding_type("missing"), None);
}

#[test]
fn refresh_stats_feeds_snapshot() {
    let mut csr = CsrWithProperties::new(vec![PropertySchema::new(
        "count".to_string(),
        0,
        DataType::Int,
    )
    .nullable(true)]);
    for i in 0..5 {
        csr.insert_for_edge(
            EdgeId(i),
            &[("count".to_string(), Value::Int((i as i32 + 1) * 10))],
            100,
        )
        .expect("stat insert should succeed");
    }
    csr.insert_for_edge(EdgeId(99), &[], 100)
        .expect("null insert should succeed");
    // Materialize the absent cell as an explicit null so the flush-time
    // statistics count it, matching the read path that serves it as null.
    csr.set_property_for_edge(EdgeId(99), "count", None, 150)
        .expect("null write should succeed");
    // Zone-map bounds are live before any refresh, but persisted counts
    // only exist after the flush-time refresh.
    let before = csr.column_stats_snapshot("count").expect("snapshot exists");
    assert_eq!(before.row_count, 6);
    assert_eq!(before.min_value, Some(Value::Int(10)));
    assert_eq!(before.max_value, Some(Value::Int(50)));
    assert_eq!(before.null_count, None);
    csr.refresh_column_stats();
    let after = csr.column_stats_snapshot("count").expect("snapshot exists");
    assert_eq!(after.row_count, 6);
    assert_eq!(after.min_value, Some(Value::Int(10)));
    assert_eq!(after.max_value, Some(Value::Int(50)));
    assert_eq!(after.null_count, Some(1));
    assert!(after.distinct_count.is_some());
    assert!(csr.column_stats_snapshot("missing").is_none());
}

#[test]
fn truncated_payloads_are_rejected() {
    let mut csr = CsrWithProperties::new(schema());
    csr.insert_for_edge(
        EdgeId(10),
        &[("weight".to_string(), Value::Double(1.0))],
        100,
    )
    .unwrap();
    let bytes = csr.dump();
    assert!(!bytes.is_empty());
    for cut in [1, 5, 9, bytes.len() / 2, bytes.len() - 1] {
        let mut loaded = CsrWithProperties::new(schema());
        assert!(
            loaded.load(&bytes[..cut]).is_err(),
            "cut at {} must fail",
            cut
        );
    }
    let mut empty = CsrWithProperties::new(schema());
    assert!(empty.load(&[]).is_err());
}

#[test]
fn release_never_admits_virgin_rows() {
    let mut csr = CsrWithProperties::new(schema());
    csr.release_row(0);
    csr.release_row(999);
    assert!(csr.edge_ids().next().is_none());
    let eid = EdgeId(11);
    csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let row = csr.get_row_for_edge(eid).expect("row exists");
    csr.release_row(row);
    assert!(csr.get_row_for_edge(eid).is_none());
    // Releasing the same used row twice never duplicates free slots.
    csr.release_row(row);
}

#[test]
fn stable_column_ids_survive_column_drop() {
    let mut csr = CsrWithProperties::new(vec![
        PropertySchema::new("a".to_string(), 0, DataType::Int),
        PropertySchema::new("b".to_string(), 1, DataType::Int),
        PropertySchema::new("c".to_string(), 2, DataType::Int),
    ]);
    let eid = EdgeId(1);
    csr.insert_for_edge(
        eid,
        &[
            ("a".to_string(), Value::Int(1)),
            ("b".to_string(), Value::Int(2)),
            ("c".to_string(), Value::Int(3)),
        ],
        100,
    )
    .unwrap();
    let id_c = csr.get_property_id("c").expect("c id exists");
    csr.remove_property("a").unwrap();
    // Stored undo parameters keyed by stable id still address column c.
    csr.set_property_by_id_for_edge(eid, id_c, Some(Value::Int(30)), 110)
        .expect("stable id must survive column drop");
    let got = csr.get_by_edge_id(eid, 110).expect("row readable");
    assert!(got
        .iter()
        .any(|(k, v)| k == "c" && v == &Some(Value::Int(30))));
    // Dropped columns stay unknown by name and by stale position.
    assert!(csr.get_property_id("a").is_none());
}

#[test]
fn physical_projection_ignores_row_visibility() {
    let mut csr = CsrWithProperties::new(schema());
    let eid = EdgeId(21);
    csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    csr.mark_deleted(eid, 150);
    assert!(csr.get_projected_by_edge_id(eid, 200, None).is_none());
    let physical = csr
        .get_projected_physical_by_edge_id(eid, 200, None)
        .expect("physical row survives");
    assert_eq!(physical.len(), 2);
}

#[test]
fn sparse_edge_map_skips_untouched_segments() {
    let mut csr = CsrWithProperties::new(schema());
    csr.map_insert(EdgeId(5000), 0).unwrap();
    csr.map_insert(EdgeId(5001), 1).unwrap();
    assert_eq!(csr.mapped_row(EdgeId(5000)), Some(0));
    assert_eq!(csr.mapped_row(EdgeId(5001)), Some(1));
    assert!(csr.mapped_row(EdgeId(0)).is_none());
    assert!(csr.mapped_row(EdgeId(4096)).is_none());
    // Untouched segments hold no allocation: one segment, not five thousand ids.
    assert_eq!(
        csr.edge_map_memory_bytes(),
        CsrWithProperties::EDGE_MAP_SEGMENT_ROWS * 4
    );
    assert_eq!(csr.nonempty_map_segments(), vec![4]);
    let mapped: Vec<(EdgeId, u32)> = csr.edge_mappings().collect();
    assert_eq!(mapped, vec![(EdgeId(5000), 0), (EdgeId(5001), 1)]);
    assert_eq!(csr.edge_mappings_in_segment(0).count(), 0);
    assert_eq!(csr.edge_mappings_in_segment(4).count(), 2);

    // Emptying the only touched segment releases it.
    csr.map_remove(EdgeId(5000));
    assert_eq!(
        csr.edge_map_memory_bytes(),
        CsrWithProperties::EDGE_MAP_SEGMENT_ROWS * 4
    );
    assert_eq!(csr.nonempty_map_segments(), vec![4]);
    csr.map_remove(EdgeId(5001));
    assert_eq!(csr.edge_map_memory_bytes(), 0);
    assert!(csr.nonempty_map_segments().is_empty());
    assert!(csr.mapped_row(EdgeId(5000)).is_none());
}
