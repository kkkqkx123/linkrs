#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;
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
}
