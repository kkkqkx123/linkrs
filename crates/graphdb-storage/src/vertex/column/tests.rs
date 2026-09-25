#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;
    use graphdb_core::types::Timestamp;
    use graphdb_core::{ArrayTypeInfo, StructTypeInfo};
    use graphdb_core::{DataType, Value};

    #[test]
    fn test_column_basic() {
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);

        col.set(0, Some(&Value::Int(25))).unwrap();
        col.set(1, Some(&Value::Int(30))).unwrap();
        col.set(2, None).unwrap();

        assert_eq!(col.get(0), Some(Value::Int(25)));
        assert_eq!(col.get(1), Some(Value::Int(30)));
        assert!(col.is_null(2));
        assert_eq!(col.len(), 3);
    }

    #[test]
    fn test_column_string() {
        let mut col = Column::new("name".to_string(), 0, DataType::String, false);

        col.set(0, Some(&Value::string("Alice"))).unwrap();
        col.set(1, Some(&Value::string("Bob"))).unwrap();

        assert_eq!(col.get(0), Some(Value::string("Alice")));
        assert_eq!(col.get(1), Some(Value::string("Bob")));
        assert_eq!(col.len(), 2);
    }

    #[test]
    fn test_column_store_batch_reads() {
        let mut store = ColumnStore::new();

        store.add_column("name".to_string(), DataType::String, false);
        store.add_column("age".to_string(), DataType::Int, true);

        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::string("Alice")),
                    ("age".to_string(), Value::Int(30)),
                ],
            )
            .unwrap();
        store
            .set(
                1,
                &[
                    ("name".to_string(), Value::string("Bob")),
                    ("age".to_string(), Value::Int(25)),
                ],
            )
            .unwrap();
        store
            .set(2, &[("name".to_string(), Value::string("Carol"))])
            .unwrap();

        // Full batch read, aligned with input order.
        let all = store.get_batch_at_ts(&[1, 0, 2], 100);
        assert_eq!(all.len(), 3);
        assert_eq!(
            all[0].iter().find(|(n, _)| n == "name").unwrap().1,
            Some(Value::string("Bob"))
        );
        assert_eq!(all[1][1], ("age".to_string(), Some(Value::Int(30))));
        assert_eq!(all[2][1], ("age".to_string(), None));

        // Projected batch read only touches the requested columns.
        let projected = store.get_projected_batch_at_ts(&[0, 1], &["age".to_string()], 100);
        assert_eq!(projected.len(), 2);
        assert_eq!(
            projected[0],
            vec![("age".to_string(), Some(Value::Int(30)))]
        );
        assert_eq!(
            projected[1],
            vec![("age".to_string(), Some(Value::Int(25)))]
        );
    }

    #[test]
    fn test_column_store() {
        let mut store = ColumnStore::new();

        store.add_column("name".to_string(), DataType::String, false);
        store.add_column("age".to_string(), DataType::Int, true);

        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::string("Alice")),
                    ("age".to_string(), Value::Int(30)),
                ],
            )
            .unwrap();

        store
            .set(
                1,
                &[
                    ("name".to_string(), Value::string("Bob")),
                    ("age".to_string(), Value::Int(25)),
                ],
            )
            .unwrap();

        assert_eq!(
            store.get_column("age").and_then(|col| col.get(0)),
            Some(Value::Int(30))
        );
        assert_eq!(
            store.get_column("name").and_then(|col| col.get(1)),
            Some(Value::string("Bob"))
        );
    }

    #[test]
    fn test_column_store_remove_and_rename() {
        let mut store = ColumnStore::new();

        store.add_column("name".to_string(), DataType::String, false);
        store.add_column("age".to_string(), DataType::Int, true);

        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::string("Alice")),
                    ("age".to_string(), Value::Int(30)),
                ],
            )
            .unwrap();

        store
            .rename_column("age", "years".to_string())
            .expect("rename should succeed");
        assert!(store.get_column("age").is_none());
        assert_eq!(
            store.get_column("years").and_then(|col| col.get(0)),
            Some(Value::Int(30))
        );

        store.remove_column("name").expect("remove should succeed");
        assert!(store.get_column("name").is_none());
        assert_eq!(store.column_count(), 1);
        assert_eq!(
            store.get_column("years").and_then(|col| col.get(0)),
            Some(Value::Int(30))
        );
    }

    #[test]
    fn test_fixed_width_multiple_types() {
        let mut col = Column::new("mixed".to_string(), 0, DataType::BigInt, false);
        col.set(0, Some(&Value::BigInt(100))).unwrap();
        col.set(1, Some(&Value::BigInt(200))).unwrap();
        assert_eq!(col.get(0), Some(Value::BigInt(100)));
        assert_eq!(col.get(1), Some(Value::BigInt(200)));
        assert_eq!(col.len(), 2);

        let mut col2 = Column::new("flag".to_string(), 1, DataType::Bool, true);
        col2.set(0, Some(&Value::Bool(true))).unwrap();
        col2.set(1, Some(&Value::Bool(false))).unwrap();
        col2.set(2, None).unwrap();
        assert_eq!(col2.get(0), Some(Value::Bool(true)));
        assert_eq!(col2.get(1), Some(Value::Bool(false)));
        assert!(col2.is_null(2));
    }

    #[test]
    fn test_flush_and_reload_fixed() {
        let mut col = Column::new("val".to_string(), 0, DataType::Int, true);
        col.set(0, Some(&Value::Int(10))).unwrap();
        col.set(1, Some(&Value::Int(20))).unwrap();
        col.set(2, None).unwrap();

        let (data, offsets, bitmap) = col.get_flush_data();
        assert!(offsets.is_empty());

        let mut restored = Column::new("val".to_string(), 0, DataType::Int, true);
        restored.load_data_from_raw(data, Vec::new(), bitmap.map(|b| b.into_vec()), col.len());

        assert_eq!(restored.get(0), Some(Value::Int(10)));
        assert_eq!(restored.get(1), Some(Value::Int(20)));
        assert!(restored.is_null(2));
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn test_flush_and_reload_variable() {
        let mut col = Column::new("name".to_string(), 0, DataType::String, true);
        col.set(0, Some(&Value::string("Hello"))).unwrap();
        col.set(1, Some(&Value::string("World"))).unwrap();
        col.set(2, None).unwrap();

        let (data, offsets, bitmap) = col.get_flush_data();
        assert!(!offsets.is_empty());

        let mut restored = Column::new("name".to_string(), 0, DataType::String, true);
        restored.load_data_from_raw(data, offsets, bitmap.map(|b| b.into_vec()), 3);

        assert_eq!(restored.get(0), Some(Value::string("Hello")));
        assert_eq!(restored.get(1), Some(Value::string("World")));
        assert!(restored.is_null(2));
        assert_eq!(restored.len(), 3);
    }

    // ==================== Priority Tests ====================

    /// Test: Verify large property values (>256 bytes) are handled correctly
    #[test]
    fn test_column_set_large_string_property() {
        let mut col = Column::new("description".to_string(), 0, DataType::String, false);

        // Create a string larger than typical storage boundaries
        let large_value = "a".repeat(1000);
        col.set(0, Some(&Value::string(large_value.clone())))
            .unwrap();
        col.set(1, Some(&Value::string("short"))).unwrap();

        assert_eq!(col.get(0), Some(Value::string(large_value.clone())));
        assert_eq!(col.get(1), Some(Value::string("short")));
        assert_eq!(col.len(), 2);
    }

    /// Test: Verify updating single property doesn't affect others
    #[test]
    fn test_column_store_update_single_property_preserves_others() {
        let mut store = ColumnStore::new();
        store.add_column("name".to_string(), DataType::String, false);
        store.add_column("age".to_string(), DataType::Int, false);
        store.add_column("city".to_string(), DataType::String, false);

        // Insert initial row
        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::string("Alice")),
                    ("age".to_string(), Value::Int(30)),
                    ("city".to_string(), Value::string("NYC")),
                ],
            )
            .unwrap();

        // Update only the age property
        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::string("Alice")),
                    ("age".to_string(), Value::Int(31)),
                    ("city".to_string(), Value::string("NYC")),
                ],
            )
            .unwrap();

        // Verify all properties are correct
        assert_eq!(
            store.get_column("name").and_then(|col| col.get(0)),
            Some(Value::string("Alice"))
        );
        assert_eq!(
            store.get_column("age").and_then(|col| col.get(0)),
            Some(Value::Int(31))
        );
        assert_eq!(
            store.get_column("city").and_then(|col| col.get(0)),
            Some(Value::string("NYC"))
        );
    }

    /// Test: Verify very large property values can be stored and retrieved
    #[test]
    fn test_column_large_string_roundtrip() {
        let mut col = Column::new("data".to_string(), 0, DataType::String, false);

        // Test different sizes around potential boundaries
        let sizes = [255, 256, 257, 1000, 10000];
        for (idx, size) in sizes.iter().enumerate() {
            let value = format!("x-{}", "a".repeat(*size));
            col.set(idx, Some(&Value::string(value.clone()))).unwrap();
            assert_eq!(
                col.get(idx),
                Some(Value::string(value)),
                "Failed at size {}",
                size
            );
        }
    }

    /// Test: Verify string column with mixed null and non-null values
    #[test]
    fn test_column_string_with_nulls() {
        let mut col = Column::new("text".to_string(), 0, DataType::String, true);

        col.set(0, Some(&Value::string("hello"))).unwrap();
        col.set(1, None).unwrap();
        col.set(2, Some(&Value::string("world"))).unwrap();
        col.set(3, None).unwrap();

        assert_eq!(col.get(0), Some(Value::string("hello")));
        assert!(col.is_null(1));
        assert_eq!(col.get(2), Some(Value::string("world")));
        assert!(col.is_null(3));
        assert_eq!(col.null_count(), 2);
    }

    /// the O(1) `null_count` counter must stay in sync with the null
    /// bitmap through set / re-set / resize operations.
    #[test]
    fn test_null_count_counter_tracks_bitmap() {
        let mut col = Column::new("text".to_string(), 0, DataType::String, true);

        col.set(0, Some(&Value::string("a"))).unwrap();
        col.set(1, None).unwrap();
        col.set(2, Some(&Value::string("b"))).unwrap();
        col.set(3, None).unwrap();
        assert_eq!(col.null_count(), 2);

        // Flip existing null → non-null and non-null → null.
        col.set(1, Some(&Value::string("c"))).unwrap();
        col.set(2, None).unwrap();
        assert_eq!(col.null_count(), 2);

        // Grow via resize: new rows are null.
        col.resize(6);
        assert_eq!(col.null_count(), 4);

        // Setting values into grown rows.
        col.set(4, Some(&Value::string("d"))).unwrap();
        assert_eq!(col.null_count(), 3);

        let expected = col.null_bitmap().map(|b| b.count_ones()).unwrap_or(0);
        assert_eq!(col.null_count(), expected);

        col.clear();
        assert_eq!(col.null_count(), 0);
    }

    /// Test: Verify integer column type conversions and boundaries
    #[test]
    fn test_column_integer_types_boundaries() {
        let mut col_small = Column::new("small".to_string(), 0, DataType::SmallInt, false);
        col_small.set(0, Some(&Value::SmallInt(i16::MAX))).unwrap();
        col_small.set(1, Some(&Value::SmallInt(i16::MIN))).unwrap();
        assert_eq!(col_small.get(0), Some(Value::SmallInt(i16::MAX)));
        assert_eq!(col_small.get(1), Some(Value::SmallInt(i16::MIN)));

        let mut col_big = Column::new("big".to_string(), 0, DataType::BigInt, false);
        col_big.set(0, Some(&Value::BigInt(i64::MAX))).unwrap();
        col_big.set(1, Some(&Value::BigInt(i64::MIN))).unwrap();
        assert_eq!(col_big.get(0), Some(Value::BigInt(i64::MAX)));
        assert_eq!(col_big.get(1), Some(Value::BigInt(i64::MIN)));
    }

    /// Test: Verify float/double precision preservation
    #[test]
    fn test_column_float_precision() {
        let mut col_f = Column::new("float_val".to_string(), 0, DataType::Float, false);
        let f_value = 1.5_f32;
        col_f.set(0, Some(&Value::Float(f_value))).unwrap();
        assert_eq!(col_f.get(0), Some(Value::Float(f_value)));

        let mut col_d = Column::new("double_val".to_string(), 0, DataType::Double, false);
        let d_value = std::f64::consts::PI;
        col_d.set(0, Some(&Value::Double(d_value))).unwrap();
        assert_eq!(col_d.get(0), Some(Value::Double(d_value)));
    }

    /// Test: Verify column resize operation maintains data integrity
    #[test]
    fn test_column_resize_maintains_data() {
        let mut col = Column::new("num".to_string(), 0, DataType::Int, false);
        col.set(0, Some(&Value::Int(10))).unwrap();
        col.set(1, Some(&Value::Int(20))).unwrap();
        col.set(2, Some(&Value::Int(30))).unwrap();

        // Simulate resize operation
        col.resize(5);
        assert_eq!(col.len(), 5);

        // Verify original data is intact
        assert_eq!(col.get(0), Some(Value::Int(10)));
        assert_eq!(col.get(1), Some(Value::Int(20)));
        assert_eq!(col.get(2), Some(Value::Int(30)));
    }

    // ==================== Priority Encoding Tests ====================

    /// Test: Column with repetitive integer values (RLE compression eligible)
    #[test]
    fn test_column_repetitive_integer_values() {
        let mut col = Column::new("status".to_string(), 0, DataType::Int, false);

        // Insert repetitive values that could benefit from RLE
        for i in 0..100 {
            let value = match i % 3 {
                0 => Value::Int(1),
                1 => Value::Int(2),
                _ => Value::Int(3),
            };
            col.set(i, Some(&value)).unwrap();
        }

        // Verify all values are stored correctly
        for i in 0..100 {
            let expected = match i % 3 {
                0 => Value::Int(1),
                1 => Value::Int(2),
                _ => Value::Int(3),
            };
            assert_eq!(col.get(i), Some(expected));
        }
    }

    /// Test: String column with low cardinality (Dictionary compression eligible)
    #[test]
    fn test_column_low_cardinality_strings() {
        let mut col = Column::new("category".to_string(), 0, DataType::String, false);

        let categories = ["A", "B", "C", "A", "B", "C"];

        // Insert low cardinality strings
        for (i, category) in categories.iter().enumerate() {
            col.set(i, Some(&Value::string(category))).unwrap();
        }

        // Verify all values are stored and retrievable
        for (i, expected_category) in categories.iter().enumerate() {
            let value = col.get(i);
            assert_eq!(value, Some(Value::string(expected_category)));
        }
    }

    /// Test: Numeric column suitable for bitpacking
    #[test]
    fn test_column_small_range_integers() {
        let mut col = Column::new("priority".to_string(), 0, DataType::Int, false);

        // Insert values with small range [0-15] - good for bitpacking
        for i in 0..256 {
            let value = Value::Int((i % 16) as i32);
            col.set(i, Some(&value)).unwrap();
        }

        // Verify all values are correctly preserved
        for i in 0..256 {
            let expected = Value::Int((i % 16) as i32);
            assert_eq!(col.get(i), Some(expected));
        }
    }

    /// Test: Long string column suitable for FSST compression
    #[test]
    fn test_column_long_strings_compression() {
        let mut col = Column::new("description".to_string(), 0, DataType::String, false);

        let long_strings = [
            "The quick brown fox jumps over the lazy dog",
            "A Rust programming language feature",
            "GraphDB storage compression techniques",
            "The quick brown fox jumps over the lazy dog", // Repetition
            "Efficient data compression algorithms",
        ];

        // Insert long strings
        for (i, s) in long_strings.iter().enumerate() {
            col.set(i, Some(&Value::string(s))).unwrap();
        }

        // Verify retrieval works correctly
        for (i, expected_str) in long_strings.iter().enumerate() {
            assert_eq!(col.get(i), Some(Value::string(expected_str)));
        }
    }

    /// Test: i64 boundary values
    #[test]
    fn test_column_i64_boundaries() {
        let mut col = Column::new("bigint_val".to_string(), 0, DataType::BigInt, false);

        // Test MAX and MIN values
        col.set(0, Some(&Value::BigInt(i64::MAX))).unwrap();
        col.set(1, Some(&Value::BigInt(i64::MIN))).unwrap();
        col.set(2, Some(&Value::BigInt(0))).unwrap();

        assert_eq!(col.get(0), Some(Value::BigInt(i64::MAX)));
        assert_eq!(col.get(1), Some(Value::BigInt(i64::MIN)));
        assert_eq!(col.get(2), Some(Value::BigInt(0)));
    }

    /// Test: Empty string handling
    #[test]
    fn test_column_empty_string() {
        let mut col = Column::new("text".to_string(), 0, DataType::String, false);

        // Test empty string
        col.set(0, Some(&Value::string(""))).unwrap();
        col.set(1, Some(&Value::string("normal"))).unwrap();

        assert_eq!(col.get(0), Some(Value::string("")));
        assert_eq!(col.get(1), Some(Value::string("normal")));
    }

    /// Test: Special characters in strings
    #[test]
    fn test_column_special_characters() {
        let mut col = Column::new("special".to_string(), 0, DataType::String, false);

        let special_strings = [
            "\n\t\r",     // Whitespace
            "\\\"'",      // Quotes and backslash
            "你好世界🌍", // Unicode and emoji
            "\0null",     // Control character
        ];

        for (idx, s) in special_strings.iter().enumerate() {
            col.set(idx, Some(&Value::string(s))).unwrap();
            assert_eq!(col.get(idx), Some(Value::string(s)));
        }
    }

    /// Test: Float special values
    #[test]
    fn test_column_float_special_values() {
        let mut col = Column::new("float_val".to_string(), 0, DataType::Float, false);

        // Test normal, zero, negative
        col.set(0, Some(&Value::Float(0.0))).unwrap();
        col.set(1, Some(&Value::Float(-1.5))).unwrap();
        col.set(2, Some(&Value::Float(f32::MAX))).unwrap();
        col.set(3, Some(&Value::Float(f32::MIN))).unwrap();

        assert_eq!(col.get(0), Some(Value::Float(0.0)));
        assert_eq!(col.get(1), Some(Value::Float(-1.5)));
        assert_eq!(col.get(2), Some(Value::Float(f32::MAX)));
        assert_eq!(col.get(3), Some(Value::Float(f32::MIN)));
    }

    #[test]
    fn test_versioned_writes_keep_before_images() {
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);

        // Insert at ts=10, then two updates at increasing timestamps.
        col.set_versioned(0, Some(&Value::Int(1)), 10).unwrap();
        col.set_versioned(0, Some(&Value::Int(2)), 20).unwrap();
        col.set_versioned(0, Some(&Value::Int(3)), 30).unwrap();

        // Current value is the latest.
        assert_eq!(col.get(0), Some(Value::Int(3)));
        // Snapshot reads resolve the visible version per timestamp.
        assert_eq!(col.get_at_ts(0, 30), Some(Value::Int(3)));
        assert_eq!(col.get_at_ts(0, 29), Some(Value::Int(2)));
        assert_eq!(col.get_at_ts(0, 20), Some(Value::Int(2)));
        assert_eq!(col.get_at_ts(0, 19), Some(Value::Int(1)));
        assert_eq!(col.get_at_ts(0, 10), Some(Value::Int(1)));
        // Before the row existed the value is null.
        assert_eq!(col.get_at_ts(0, 9), None);

        // GC removes versions older than the minimum active snapshot.
        let removed = col.gc_versions(21);
        assert!(removed >= 1, "old versions should be reclaimed");
        assert_eq!(
            col.get_at_ts(0, 29),
            Some(Value::Int(2)),
            "still-visible version survives"
        );
        assert_eq!(col.get(0), Some(Value::Int(3)));
    }

    #[test]
    fn test_versioned_null_and_string_types() {
        let mut col = Column::new("name".to_string(), 0, DataType::String, true);

        col.set_versioned(0, Some(&Value::string("alice")), 10)
            .unwrap();
        col.set_versioned(0, None, 20).unwrap();
        col.set_versioned(0, Some(&Value::string("bob")), 30)
            .unwrap();

        assert_eq!(col.get_at_ts(0, 10), Some(Value::string("alice")));
        assert_eq!(col.get_at_ts(0, 20), None, "null before-image");
        assert_eq!(col.get_at_ts(0, 30), Some(Value::string("bob")));
        assert_eq!(col.get_at_ts(0, 25), None);
    }

    #[test]
    fn test_versioned_write_at_or_before_start_is_noop_range() {
        let mut col = Column::new("v".to_string(), 0, DataType::BigInt, true);
        col.set_versioned(0, Some(&Value::BigInt(7)), 100).unwrap();
        // Rollback-style write reusing the same timestamp must not create a
        // zero-length version range.
        col.set_versioned(0, Some(&Value::BigInt(9)), 100).unwrap();
        assert_eq!(col.get_at_ts(0, 100), Some(Value::BigInt(9)));
        assert_eq!(col.get_at_ts(0, 101), Some(Value::BigInt(9)));
        // The before-image at 100 was already superseded at the same ts.
        assert_eq!(col.get_at_ts(0, 99), None);
    }

    #[test]
    fn test_composite_types_use_variable_width_column() {
        // Struct/Array must never fall into FixedWidthColumn (element_size 0
        // would corrupt offsets).
        let struct_type = DataType::Struct(std::sync::Arc::new(StructTypeInfo::new(vec![(
            "city".to_string(),
            DataType::String,
        )])));
        let array_type = DataType::Array(std::sync::Arc::new(ArrayTypeInfo::new(
            DataType::Double,
            Some(3),
        )));
        for (data_type, value) in [
            (
                struct_type,
                Value::struct_(vec![("city".to_string(), Value::string("x"))]),
            ),
            (
                array_type,
                Value::array(vec![Value::Double(1.0), Value::Double(2.0)]),
            ),
            (
                DataType::FixedString(8),
                Value::FixedString("abcdef".to_string()),
            ),
            (
                DataType::VectorDense(2),
                Value::Vector(graphdb_core::value::VectorValue::dense(vec![1.0, 2.0])),
            ),
        ] {
            let mut col = Column::new("c".to_string(), 0, data_type.clone(), true);
            assert!(crate::vertex::column::is_variable_length_type(&data_type));
            col.set_versioned(0, Some(&value), 10).unwrap();
            assert_eq!(col.get_at_ts(0, 10), Some(value.clone()));
            // MVCC before-image roundtrip through the undo path.
            col.set_versioned(0, None, 20).unwrap();
            assert_eq!(col.get_at_ts(0, 10), Some(value.clone()));
            assert_eq!(col.get_at_ts(0, 20), None);
        }
    }

    #[test]
    fn test_extended_types_never_use_zero_step_fixed_column() {
        // FixedString, Decimal family and Union have no fixed element size;
        // they must route to variable-width storage.
        for data_type in [
            DataType::FixedString(4),
            DataType::Decimal128,
            DataType::Decimal {
                precision: 10,
                scale: 2,
            },
            DataType::Union(vec![DataType::Int, DataType::String]),
            DataType::Interval,
            DataType::List(Box::new(DataType::Int)),
        ] {
            assert!(
                crate::vertex::column::is_variable_length_type(&data_type),
                "{:?} must be variable-length",
                data_type
            );
            assert_eq!(crate::vertex::column::element_size(&data_type), 0);
            let col = Column::new("c".to_string(), 0, data_type, true);
            assert!(matches!(
                col.inner,
                crate::vertex::column::column::ColumnInner::Variable(_)
            ));
        }
    }

    #[test]
    fn test_clear_page_dirty_only_clears_requested_page() {
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);
        let page_rows = crate::persistence::dirty_page::ROWS_PER_PAGE;

        col.set(page_rows - 1, Some(&Value::Int(1))).unwrap();
        col.set(page_rows, Some(&Value::Int(2))).unwrap();
        col.mark_dirty(0);
        col.mark_dirty(page_rows);

        assert_eq!(col.dirty_pages(), vec![0, 1]);

        col.clear_page_dirty(0);
        assert_eq!(col.dirty_pages(), vec![1]);

        col.clear_page_dirty(1);
        assert_eq!(col.dirty_pages(), Vec::<usize>::new());
    }

    #[test]
    fn test_column_store_clear_pages_keeps_other_pages_dirty() {
        let mut store = ColumnStore::new();
        store.add_column("name".to_string(), DataType::String, false);
        store.add_column("age".to_string(), DataType::Int, true);

        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::string("Alice")),
                    ("age".to_string(), Value::Int(30)),
                ],
            )
            .unwrap();

        let pages = store.collect_dirty_pages();
        assert!(pages.iter().all(|p| p.page_id == 0));

        store.clear_pages(&[("name".to_string(), 0)]);
        let remaining = store
            .columns()
            .iter()
            .map(|col| (col.name.clone(), col.dirty_pages()))
            .collect::<Vec<_>>();
        assert_eq!(
            remaining,
            vec![
                ("name".to_string(), Vec::<usize>::new()),
                ("age".to_string(), vec![0])
            ]
        );
    }

    #[test]
    fn test_column_chunk_materialize_and_read() {
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);
        col.set_chunk_capacity(4);
        for i in 0..10 {
            col.set(i, Some(&Value::Int(i as i32))).unwrap();
        }
        col.clear_dirty();
        col.materialize_chunks();
        assert_eq!(col.chunk_count(), 3);
        // Chunk-layer reads serve every row through the active routing.
        for i in 0..10 {
            assert_eq!(col.get(i), Some(Value::Int(i as i32)));
        }
        assert_eq!(col.chunk_for_row(3).unwrap().row_offset, 0);
        assert!(col.chunk_for_row(100).is_none());
    }

    #[test]
    fn test_column_chunk_layer_read() {
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);
        col.set_chunk_capacity(4);
        for i in 0..8 {
            col.set(i, Some(&Value::Int(i as i32))).unwrap();
        }
        col.materialize_chunks();
        assert_eq!(col.get(3), Some(Value::Int(3)));
        assert_eq!(col.chunk_for_row(3).unwrap().row_offset, 0);
        assert!(col.chunk_for_row(100).is_none());
    }

    #[test]
    fn test_column_chunk_encoding_roundtrip() {
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);
        col.set_chunk_capacity(4);
        for i in 0..8 {
            col.set(i, Some(&Value::Int((i % 4) as i32))).unwrap();
        }
        col.clear_dirty();
        col.materialize_chunks();
        col.apply_encoding_to_chunks(crate::encoding::EncodingType::BitPacking, 255)
            .unwrap();
        let meta = col.chunk_encoding_metadata();
        assert_eq!(meta.len(), 2);
        // Point update inside bit width lands without full decode: the
        // chunk keeps its encoding and serves the new value.
        col.set(1, Some(&Value::Int(2))).unwrap();
        assert_eq!(col.get(1), Some(Value::Int(2)));
        assert_eq!(
            col.chunk_encoding_metadata()[0].1,
            crate::encoding::EncodingType::BitPacking
        );
    }

    #[test]
    fn test_column_chunk_constant_inplace_rules() {
        let mut col = Column::new("s".to_string(), 0, DataType::String, true);
        col.set_chunk_capacity(8);
        for i in 0..8 {
            col.set(i, Some(&Value::string("same"))).unwrap();
        }
        col.clear_dirty();
        col.materialize_chunks();
        col.apply_encoding_to_chunks(crate::encoding::EncodingType::Constant, 255)
            .unwrap();
        // Equal value is a noop; different value lands in the overlay while
        // the chunk keeps its constant encoding.
        col.set(0, Some(&Value::string("same"))).unwrap();
        assert_eq!(col.get(0), Some(Value::string("same")));
        col.set(1, Some(&Value::string("other"))).unwrap();
        assert_eq!(col.get(1), Some(Value::string("other")));
        assert_eq!(
            col.chunk_encoding_metadata()[0].1,
            crate::encoding::EncodingType::Constant
        );
    }

    #[test]
    fn test_column_store_collect_dirty_pages() {
        let mut store = ColumnStore::new();
        store.add_column("age".to_string(), DataType::Int, true);
        let col = store.get_column_mut("age").unwrap();
        col.set_chunk_capacity(4);
        for i in 0..8 {
            col.set(i, Some(&Value::Int(i as i32))).unwrap();
        }
        store.get_column_mut("age").unwrap().materialize_chunks();
        // Written rows report dirty pages; clearing resets the tracking.
        assert!(!store.collect_dirty_pages().is_empty());
        store.clear_dirty();
        assert!(store.collect_dirty_pages().is_empty());
    }

    #[test]
    fn test_gc_versions_boundary_at_cutoff() {
        // Entries ending exactly at the cutoff are no longer visible to any
        // snapshot at/after it, so they are reclaimed; newer entries survive.
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);
        col.set_versioned(0, Some(&Value::Int(1)), 10).unwrap();
        col.set_versioned(0, Some(&Value::Int(2)), 20).unwrap();
        col.set_versioned(0, Some(&Value::Int(3)), 30).unwrap();

        let removed = col.gc_versions(20);
        assert_eq!(
            removed, 1,
            "entry [10,20) ends at the cutoff and is reclaimed"
        );
        assert_eq!(
            col.get_at_ts(0, 15),
            None,
            "reclaimed history is unreadable"
        );
        assert_eq!(col.get_at_ts(0, 25), Some(Value::Int(2)));
        assert_eq!(col.get(0), Some(Value::Int(3)));
    }

    #[test]
    fn test_gc_versions_at_max_keeps_single_baseline() {
        // With no active snapshot the watermark is MAX: every before-image is
        // unreachable, but one baseline entry is conservatively retained.
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);
        col.set_versioned(0, Some(&Value::Int(1)), 10).unwrap();
        col.set_versioned(0, Some(&Value::Int(2)), 20).unwrap();
        col.set_versioned(0, Some(&Value::Int(3)), 30).unwrap();

        let removed = col.gc_versions(Timestamp::MAX);
        assert_eq!(removed, 1, "one baseline entry is retained");
        assert_eq!(col.version_chain_len(0), 1);
        assert_eq!(col.get(0), Some(Value::Int(3)));
        assert_eq!(col.get_at_ts(0, Timestamp::MAX), Some(Value::Int(3)));
    }

    #[test]
    fn test_clone_row_state_preserves_history() {
        // Row moves during vertex compaction must carry creation time and
        // before-images so snapshot reads stay intact after the remap.
        let mut src = Column::new("age".to_string(), 0, DataType::Int, true);
        src.set_versioned(0, Some(&Value::Int(1)), 10).unwrap();
        src.set_versioned(0, Some(&Value::Int(2)), 20).unwrap();

        let mut dst = Column::new("age".to_string(), 0, DataType::Int, true);
        dst.set(5, Some(&Value::Int(2))).unwrap();
        dst.clone_row_state_from(&src, 0, 5);

        assert_eq!(dst.version_chain_len(5), 1);
        assert_eq!(dst.get_at_ts(5, 15), Some(Value::Int(1)));
        assert_eq!(dst.get_at_ts(5, 25), Some(Value::Int(2)));
    }

    #[test]
    fn test_clone_row_state_never_written_row() {
        // Cloning a row that was never written must not fabricate history.
        let mut src = Column::new("age".to_string(), 0, DataType::Int, true);
        src.set_versioned(0, Some(&Value::Int(1)), 10).unwrap();

        let mut dst = Column::new("age".to_string(), 0, DataType::Int, true);
        dst.clone_row_state_from(&src, 7, 3);

        assert_eq!(dst.version_chain_len(3), 0);
        assert_eq!(dst.get_at_ts(3, 100), None);
    }

    #[test]
    fn test_fixed_string_dictionary_roundtrip_preserves_type() {
        let mut col = Column::new("code".to_string(), 0, DataType::FixedString(4), true);
        let values = ["ab", "cd", "ab", "ef"];
        for (i, s) in values.iter().enumerate() {
            col.set(i, Some(&Value::FixedString(s.to_string())))
                .unwrap();
        }
        col.apply_dictionary_encoding().unwrap();
        assert_eq!(
            col.encoding_type(),
            crate::encoding::EncodingType::Dictionary
        );
        for (i, s) in values.iter().enumerate() {
            assert_eq!(col.get(i), Some(Value::FixedString(s.to_string())));
        }
    }

    #[test]
    fn test_fixed_string_chunk_dictionary_preserves_type() {
        let mut col = Column::new("code".to_string(), 0, DataType::FixedString(4), true);
        col.set_chunk_capacity(4);
        for i in 0..8 {
            col.set(i, Some(&Value::FixedString(format!("v{}", i % 2))))
                .unwrap();
        }
        col.clear_dirty();
        col.materialize_chunks();
        col.apply_encoding_to_chunks(crate::encoding::EncodingType::Dictionary, 255)
            .unwrap();
        assert_eq!(
            col.chunk_encoding_metadata()[0].1,
            crate::encoding::EncodingType::Dictionary
        );
        for i in 0..8 {
            assert_eq!(col.get(i), Some(Value::FixedString(format!("v{}", i % 2))));
        }
    }

    #[test]
    fn test_fixed_string_selector_prefers_dictionary() {
        let selector = crate::encoding::EncodingSelector::default();
        let values: Vec<Option<Value>> = (0..100)
            .map(|i| Some(Value::FixedString(format!("s{}", i % 5))))
            .collect();
        assert_eq!(
            selector.select_for_column(&DataType::FixedString(8), &values),
            crate::encoding::EncodingType::Dictionary
        );
    }

    fn encoded_two_chunk_column() -> Column {
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);
        col.set_chunk_capacity(4);
        for i in 0..8 {
            col.set(i, Some(&Value::Int((i % 4) as i32))).unwrap();
        }
        col.clear_dirty();
        col.materialize_chunks();
        col.apply_encoding_to_chunks(crate::encoding::EncodingType::BitPacking, 255)
            .unwrap();
        assert!(col.chunk_evictable(0));
        assert!(col.chunk_evictable(1));
        col
    }

    #[test]
    fn test_evicted_point_and_batch_reads_match_resident() {
        let mut col = encoded_two_chunk_column();
        let (evicted, freed) = col.evict_cold_chunks(u64::MAX);
        assert_eq!(evicted, 2);
        assert!(freed > 0);
        assert_eq!(col.evicted_chunk_count(), 2);
        assert_eq!(col.resident_chunk_count(), 0);
        assert!(col.evicted_bytes() > 0);
        for i in 0..8 {
            assert_eq!(col.get(i), Some(Value::Int((i % 4) as i32)));
        }
        // Served from snapshots: still evicted, stats still resident.
        assert_eq!(col.evicted_chunk_count(), 2);
        assert!(col.compute_stats().is_ok());
        // Batch promotion loads each covered chunk exactly once.
        let loaded = col.ensure_resident_range(&[0, 1, 6, 7]).unwrap();
        assert_eq!(loaded, 2);
        assert_eq!(col.evicted_chunk_count(), 0);
        for i in 0..8 {
            assert_eq!(col.get(i), Some(Value::Int((i % 4) as i32)));
        }
    }

    #[test]
    fn test_overlay_chunks_never_evict() {
        let mut col = encoded_two_chunk_column();
        // Out-of-width update lands in the overlay: chunk 0 goes dirty.
        col.set(1, Some(&Value::Int(1000))).unwrap();
        assert_eq!(col.get(1), Some(Value::Int(1000)));
        assert!(!col.chunk_evictable(0));
        assert!(col.chunk_evictable(1));
        let (evicted, _) = col.evict_cold_chunks(u64::MAX);
        assert_eq!(evicted, 1);
        assert_eq!(col.evicted_chunk_count(), 1);
        assert_eq!(col.get(1), Some(Value::Int(1000)));
    }

    #[test]
    fn test_write_before_load_preserves_point_write_semantics() {
        let mut col = encoded_two_chunk_column();
        col.evict_cold_chunks(u64::MAX);
        assert_eq!(col.evicted_chunk_count(), 2);
        col.set(3, Some(&Value::Int(42))).unwrap();
        assert_eq!(col.evicted_chunk_count(), 1);
        assert_eq!(col.get(3), Some(Value::Int(42)));
        assert_eq!(col.get(0), Some(Value::Int(0)));
    }

    #[test]
    fn test_resident_accounting_splits_snapshot_bytes() {
        let mut col = encoded_two_chunk_column();
        let resident_before = col.resident_memory_usage();
        col.evict_cold_chunks(u64::MAX);
        assert!(col.resident_memory_usage() < resident_before);
        // Spilled snapshots leave the heap: heap memory drops while the
        // evicted payload footprint stays observable.
        assert!(col.memory_usage() < resident_before);
        assert!(col.evicted_bytes() > 0);
    }

    #[test]
    fn test_evict_double_confirm_abandons_chunk_written_during_selection() {
        // Selection sees two evictable chunks; a write landing before the
        // free rechecks evictability inside `evict_chunk` and abandons it.
        let mut col = encoded_two_chunk_column();
        assert!(col.chunk_evictable(0));
        assert!(col.chunk_evictable(1));
        col.set(1, Some(&Value::Int(1000))).unwrap();
        assert!(!col.chunk_evictable(0));
        assert_eq!(col.evict_chunk(0).unwrap(), 0);
        let (evicted, _) = col.evict_cold_chunks(u64::MAX);
        assert_eq!(evicted, 1);
        assert_eq!(col.evicted_chunk_count(), 1);
        assert_eq!(col.get(1), Some(Value::Int(1000)));
    }

    #[test]
    fn test_quota_segments_large_eviction_without_changing_totals() {
        let mut full = encoded_two_chunk_column();
        let (full_evicted, full_freed) = full.evict_cold_chunks(u64::MAX);
        let mut segmented = encoded_two_chunk_column();
        let (seg_evicted, seg_freed, segments) =
            segmented.evict_cold_chunks_with_quota(u64::MAX, 1);
        assert_eq!((seg_evicted, seg_freed), (full_evicted, full_freed));
        assert!(segments >= 2);
        for i in 0..8 {
            assert_eq!(segmented.get(i), Some(Value::Int((i % 4) as i32)));
        }
        assert!(segmented.compute_stats().is_ok());
    }

    #[test]
    fn test_pressure_scan_matches_resident_with_quota_segmented_loads() {
        // Working set larger than the per-segment quota: evict everything,
        // then reload in quota-capped segments; results match resident and
        // statistics stay usable after swap-out.
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);
        col.set_chunk_capacity(4);
        let rows = 64usize;
        for i in 0..rows {
            col.set(i, Some(&Value::Int((i % 8) as i32))).unwrap();
        }
        col.clear_dirty();
        col.materialize_chunks();
        col.apply_encoding_to_chunks(crate::encoding::EncodingType::BitPacking, 255)
            .unwrap();
        let expected: Vec<Option<Value>> = (0..rows).map(|i| col.get(i)).collect();
        let (evicted, freed, _) = col.evict_cold_chunks_with_quota(u64::MAX, u64::MAX);
        assert!(evicted > 0 && freed > 0);
        for (i, value) in expected.iter().enumerate() {
            assert_eq!(col.get(i).as_ref(), value.as_ref());
        }
        assert!(col.compute_stats().is_ok());
        let all_rows: Vec<usize> = (0..rows).collect();
        let mut remaining = all_rows.clone();
        let mut loaded_total = 0usize;
        while !remaining.is_empty() {
            let (loaded, rest) = col.ensure_resident_range_with_quota(&remaining, 2).unwrap();
            assert!(loaded > 0 && loaded <= 4);
            loaded_total += loaded;
            remaining = rest;
        }
        assert!(loaded_total >= 4);
        assert_eq!(col.evicted_chunk_count(), 0);
        for (i, value) in expected.iter().enumerate() {
            assert_eq!(col.get(i).as_ref(), value.as_ref());
        }
    }

    #[test]
    fn fixed_string_over_length_is_rejected() {
        let mut col = Column::new("code".to_string(), 0, DataType::FixedString(3), false);
        assert!(col
            .set(0, Some(&Value::FixedString("abc".to_string())))
            .is_ok());
        assert!(col
            .set(1, Some(&Value::FixedString("abcd".to_string())))
            .is_err());
        assert_eq!(col.get(0), Some(Value::FixedString("abc".to_string())));
    }

    #[test]
    fn complex_length_summary_prunes_equality() {
        use crate::cursor::PredicateRange;
        use crate::cursor::ScanPredicate;
        let mut col = Column::new(
            "tags".to_string(),
            0,
            DataType::List(Box::new(DataType::Int)),
            true,
        );
        let v2 = Value::List(Box::new(graphdb_core::value::list::List::from_vec(vec![
            Value::Int(1),
            Value::Int(2),
        ])));
        let v3 = Value::List(Box::new(graphdb_core::value::list::List::from_vec(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ])));
        col.set(0, Some(&v2)).unwrap();
        col.set(1, Some(&v2)).unwrap();
        let bounds = col.complex_len_bounds(0).expect("summary exists");
        assert_eq!(bounds, (2, 2));
        let probe_outside = Value::List(Box::new(graphdb_core::value::list::List::from_vec(vec![
            Value::Int(9),
            Value::Int(8),
            Value::Int(7),
            Value::Int(6),
            Value::Int(5),
        ])));
        let range = PredicateRange {
            column: "tags".to_string(),
            lower: Some(probe_outside.clone()),
            include_lower: true,
            upper: Some(probe_outside),
            include_upper: true,
        };
        assert_eq!(range.equality_len(), Some(5));
        let mut store = ColumnStore::new();
        store.add_column(
            "tags".to_string(),
            DataType::List(Box::new(DataType::Int)),
            true,
        );
        store
            .set_versioned(0, &[("tags".to_string(), v3.clone())], 10)
            .unwrap();
        store
            .set_versioned(1, &[("tags".to_string(), v3.clone())], 10)
            .unwrap();
        let _ = ScanPredicate::ColumnEqual {
            column: "tags".to_string(),
            value: v3,
        };
    }

    #[test]
    fn hll_distinguishes_equal_sized_complex_values() {
        use crate::stats::HyperLogLog;
        let a = Value::List(Box::new(graphdb_core::value::list::List::from_vec(vec![
            Value::Int(1),
            Value::Int(2),
        ])));
        let b = Value::List(Box::new(graphdb_core::value::list::List::from_vec(vec![
            Value::Int(3),
            Value::Int(4),
        ])));
        let mut ha = HyperLogLog::new();
        ha.add_value(&a);
        let mut hb = HyperLogLog::new();
        hb.add_value(&b);
        assert_ne!(ha.registers(), hb.registers());
    }

    #[test]
    fn overlay_half_full_triggers_early_merge_signal() {
        use crate::encoding::EncodingType;
        let mut col = Column::new("v".to_string(), 0, DataType::Int, true);
        for i in 0..8 {
            col.set(i, Some(&Value::Int(i as i32))).unwrap();
        }
        col.clear_dirty();
        col.materialize_chunks();
        col.apply_encoding_to_chunks(EncodingType::BitPacking, 255)
            .unwrap();
        assert!(col.has_chunks());
        let hot = crate::encoding::selector::EncodingThresholds::default().hot_update_threshold;
        for i in 0..1100 {
            let row = (i % 8) as usize;
            col.set_versioned(row, Some(&Value::Int(1000 + i as i32)), 100 + i as u64)
                .unwrap();
        }
        assert!(col.zone_needs_exact_rebuild());
        assert!(col.maybe_rebuild_zone_maps_exact());
        assert!(!col.zone_needs_exact_rebuild());
        let _ = col.pending_recode_chunks(hot);
    }

    fn list_of(values: Vec<Value>) -> Value {
        Value::List(Box::new(graphdb_core::value::list::List::from_vec(values)))
    }

    fn store_with_list_rows(rows: Vec<Value>) -> ColumnStore {
        let mut store = ColumnStore::new();
        store.add_column(
            "v".to_string(),
            DataType::List(Box::new(DataType::Int)),
            true,
        );
        for (row, value) in rows.into_iter().enumerate() {
            store
                .set_versioned(row, &[("v".to_string(), value)], 10)
                .unwrap();
        }
        store
    }

    fn point_range(column: &str, probe: Value) -> crate::cursor::PredicateRange {
        crate::cursor::PredicateRange {
            column: column.to_string(),
            lower: Some(probe.clone()),
            include_lower: true,
            upper: Some(probe),
            include_upper: true,
        }
    }

    #[test]
    fn nested_list_equality_prunes_disjoint_leaves() {
        let store = store_with_list_rows(vec![list_of(vec![Value::Int(1), Value::Int(2)])]);
        let probe = list_of(vec![Value::Int(3), Value::Int(4)]);
        assert_eq!(point_range("v", probe.clone()).equality_len(), Some(2));
        assert!(!store.zone_prunes_in(0, &point_range("v", probe)));
    }

    #[test]
    fn nested_list_equality_keeps_matching_chunk() {
        let present = list_of(vec![Value::Int(1), Value::Int(2)]);
        let store = store_with_list_rows(vec![present.clone()]);
        assert!(store.zone_prunes_in(0, &point_range("v", present)));
    }

    #[test]
    fn nested_map_equality_prunes_disjoint_leaves() {
        use std::collections::HashMap;
        let mut fields = HashMap::new();
        fields.insert(Value::string("a"), Value::Int(1));
        let mut store = ColumnStore::new();
        store.add_column(
            "v".to_string(),
            DataType::Map(Box::new(DataType::Int)),
            true,
        );
        store
            .set_versioned(0, &[("v".to_string(), Value::Map(Box::new(fields)))], 10)
            .unwrap();
        let mut probe_fields = HashMap::new();
        probe_fields.insert(Value::string("zzz-no-such-key"), Value::Int(999));
        let probe = Value::Map(Box::new(probe_fields));
        assert!(!store.zone_prunes_in(0, &point_range("v", probe)));
    }

    #[test]
    fn nested_struct_equality_prunes_key_mismatch() {
        use crate::vertex::column::zone_map::complex_key_fp;
        use std::sync::Arc;
        let mut store = ColumnStore::new();
        store.add_column(
            "v".to_string(),
            DataType::Struct(Arc::new(StructTypeInfo {
                fields: vec![("x".to_string(), DataType::Int)],
            })),
            true,
        );
        let present = Value::Struct(Box::new(graphdb_core::StructValue::new(vec![(
            "x".to_string(),
            Value::Int(1),
        )])));
        store
            .set_versioned(0, &[("v".to_string(), present.clone())], 10)
            .unwrap();
        let chunk_fp = complex_key_fp(&present);
        let probe_name = (0..1000)
            .map(|i| format!("absent-key-{i}"))
            .map(|name| {
                let probe = Value::Struct(Box::new(graphdb_core::StructValue::new(vec![(
                    name.clone(),
                    Value::Int(1),
                )])));
                (probe, name)
            })
            .find(|(probe, _)| chunk_fp & complex_key_fp(probe) != complex_key_fp(probe))
            .map(|(_, name)| name)
            .expect("a disjoint bloom key exists");
        let probe = Value::Struct(Box::new(graphdb_core::StructValue::new(vec![(
            probe_name,
            Value::Int(1),
        )])));
        assert!(!store.zone_prunes_in(0, &point_range("v", probe)));
    }

    #[test]
    fn nested_json_equality_prunes_disjoint_leaves() {
        let mut store = ColumnStore::new();
        store.add_column("v".to_string(), DataType::Json, true);
        let present = Value::Json(Box::new(
            graphdb_core::value::json::Json::parse(r#"{"n":1}"#).unwrap(),
        ));
        store
            .set_versioned(0, &[("v".to_string(), present)], 10)
            .unwrap();
        let probe = Value::Json(Box::new(
            graphdb_core::value::json::Json::parse(r#"{"n":2}"#).unwrap(),
        ));
        let range = point_range("v", probe.clone());
        assert_eq!(range.equality_len(), Some(7));
        assert!(!store.zone_prunes_in(0, &range));
    }

    fn unique_snapshot_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vtx-evict-{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn evict_snapshots_persist_and_reload_mapped() {
        use crate::encoding::EncodingType;
        let mut store = ColumnStore::new();
        store.add_column("v".to_string(), DataType::Int, true);
        let col = store.get_column_mut("v").expect("column exists");
        col.set_chunk_capacity(512);
        for i in 0..2000 {
            col.set(i, Some(&Value::Int(i as i32))).unwrap();
        }
        col.materialize_chunks();
        assert!(col.chunk_count() > 1);
        col.apply_encoding_to_chunks(EncodingType::BitPacking, 255)
            .unwrap();
        let released = col.evict_chunk(0).unwrap();
        assert!(released > 0);
        assert!(col.chunks[0].residency.is_evicted());

        let dir = unique_snapshot_dir("reload");
        store.flush_evict_snapshots(&dir).unwrap();
        assert!(dir.join("v.snapshot").exists());

        let col = store.get_column_mut("v").expect("column exists");
        col.ensure_all_resident().unwrap();
        assert!(col.chunks[0].residency.is_resident());
        store.load_evict_snapshots(&dir);
        let col = store.get_column_mut("v").expect("column exists");
        assert!(col.chunks[0].residency.is_evicted());
        assert_eq!(col.get(0), Some(Value::Int(0)));
        assert_eq!(col.get(511), Some(Value::Int(511)));
        assert_eq!(col.get(512), Some(Value::Int(512)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_snapshot_sidecar_keeps_chunks_resident() {
        use crate::encoding::EncodingType;
        let mut store = ColumnStore::new();
        store.add_column("v".to_string(), DataType::Int, true);
        let col = store.get_column_mut("v").expect("column exists");
        col.set_chunk_capacity(512);
        for i in 0..1000 {
            col.set(i, Some(&Value::Int(i as i32))).unwrap();
        }
        col.materialize_chunks();
        col.apply_encoding_to_chunks(EncodingType::BitPacking, 255)
            .unwrap();
        assert!(col.evict_chunk(0).unwrap() > 0);

        let dir = unique_snapshot_dir("corrupt");
        store.flush_evict_snapshots(&dir).unwrap();
        std::fs::write(dir.join("v.snapshot"), b"junk").unwrap();

        let col = store.get_column_mut("v").expect("column exists");
        col.ensure_all_resident().unwrap();
        store.load_evict_snapshots(&dir);
        let col = store.get_column_mut("v").expect("column exists");
        assert!(col.chunks[0].residency.is_resident());
        assert_eq!(col.get(0), Some(Value::Int(0)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
