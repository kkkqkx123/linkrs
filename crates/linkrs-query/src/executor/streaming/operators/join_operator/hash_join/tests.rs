use super::build_side::HashJoinBuildSide;
use super::*;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::Value;
use std::sync::Arc;

    fn chunk_from_columns(cols: Vec<Vec<Value>>) -> DataChunk {
        let names: Vec<String> = (0..cols.len()).map(|i| format!("c{i}")).collect();
        DataChunk::from_columns(cols, Arc::new(SlotLayout::from_names(&names)))
    }

    #[test]
    fn insert_chunk_accumulates_across_chunks() {
        let mut side = HashJoinBuildSide::new();
        let mut c1 = chunk_from_columns(vec![
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::string("a"), Value::string("b")],
        ]);
        side.insert_chunk(&mut c1, &[], &[]).unwrap();
        let mut c2 = chunk_from_columns(vec![vec![Value::Int(3)], vec![Value::string("c")]]);
        side.insert_chunk(&mut c2, &[], &[]).unwrap();
        assert_eq!(side.build_columns.len(), 2);
        assert_eq!(
            side.build_columns[0],
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        );
        assert_eq!(side.row_at(2), vec![Value::Int(3), Value::string("c")]);
        let indexed_rows: usize = side.index.values().map(|v| v.len()).sum();
        assert_eq!(indexed_rows, 3);
    }

    #[test]
    fn insert_chunk_column_count_mismatch_is_error() {
        let mut side = HashJoinBuildSide::new();
        let mut c1 = chunk_from_columns(vec![vec![Value::Int(1)], vec![Value::Int(2)]]);
        side.insert_chunk(&mut c1, &[], &[]).unwrap();
        let mut c2 = chunk_from_columns(vec![
            vec![Value::Int(3)],
            vec![Value::Int(4)],
            vec![Value::Int(5)],
        ]);
        let err = side.insert_chunk(&mut c2, &[], &[]).unwrap_err();
        assert!(err.to_string().contains("column count"));
        assert_eq!(side.build_columns.len(), 2);
        assert_eq!(side.build_columns[0], vec![Value::Int(1)]);
    }

    #[test]
    fn insert_chunk_consumes_selection_in_place() {
        // Single entry point: an attached selection is consumed without a
        // prior materialization, and only visible rows land in the store.
        let mut side = HashJoinBuildSide::new();
        let mut chunk = chunk_from_columns(vec![
            vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            vec![Value::string("a"), Value::string("b"), Value::string("c")],
        ])
        .with_selection(vec![0, 2]);
        side.insert_chunk(&mut chunk, &[], &[]).unwrap();
        assert_eq!(side.row_count(), 2);
        assert_eq!(side.build_columns[0], vec![Value::Int(1), Value::Int(3)]);
        assert!(chunk.selection().is_none());
        assert!(chunk.rows.is_empty());
        let indexed_rows: usize = side.index.values().map(|v| v.len()).sum();
        assert_eq!(indexed_rows, 2);
    }

    #[test]
    #[should_panic(expected = "row/column count mismatch")]
    fn insert_chunk_rejects_schema_less_chunk() {
        // Rows carrying values that no schema column can address would be
        // silently dropped from the build side; the invariant guard must fire.
        let mut side = HashJoinBuildSide::new();
        let mut chunk = DataChunk::new_with_layout(
            vec![vec![Value::Int(1)]],
            Arc::new(SlotLayout::from_names(&[])),
        );
        let _ = side.insert_chunk(&mut chunk, &[], &[]);
    }

    #[test]
    fn insert_chunk_typed_layout_matches_row_path() {
        // Same logical chunk built twice: once with the typed layout, once
        // without. Both build paths must land byte-identical values,
        // including hidden rows skipped via selection and NULL cells.
        use linkrs_core::value::NullType;
        let cols = || {
            vec![
                vec![Value::Int(1), Value::Int(2), Value::Int(3)],
                vec![
                    Value::string("a"),
                    Value::Null(NullType::Null),
                    Value::string("c"),
                ],
            ]
        };
        let mut typed_chunk = chunk_from_columns(cols());
        typed_chunk.build_typed_columns(true);
        assert!(typed_chunk.typed_columns.is_some());
        let mut typed_chunk = typed_chunk.with_selection(vec![0, 2]);
        let mut rows_chunk = chunk_from_columns(cols()).with_selection(vec![0, 2]);

        let mut typed_side = HashJoinBuildSide::new();
        typed_side
            .insert_chunk(&mut typed_chunk, &[], &[])
            .expect("typed insert");
        let mut rows_side = HashJoinBuildSide::new();
        rows_side
            .insert_chunk(&mut rows_chunk, &[], &[])
            .expect("rows insert");

        assert_eq!(typed_side.row_count(), 2);
        assert_eq!(typed_side.row_count(), rows_side.row_count());
        for idx in 0..typed_side.row_count() as u32 {
            assert_eq!(typed_side.row_at(idx), rows_side.row_at(idx));
        }
        assert_eq!(
            typed_side.row_at(1),
            vec![Value::Int(3), Value::string("c")]
        );
        // The typed layout is consumed by the build, not dropped.
        assert!(typed_chunk.typed_columns.is_none());
    }

    #[test]
    fn insert_chunk_typed_consume_skips_wasted_build_count() {
        use std::sync::atomic::Ordering;
        // Typed layout consumed by the build: no wasted-build record.
        let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
        let mut chunk = chunk_from_columns(vec![
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::string("a"), Value::string("b")],
        ])
        .with_columnar_stats(Arc::clone(&stats));
        chunk.build_typed_columns(true);
        let mut side = HashJoinBuildSide::new();
        side.insert_chunk(&mut chunk, &[], &[])
            .expect("typed insert");
        assert_eq!(stats.columnar_wasted_builds.load(Ordering::Relaxed), 0);

        // Typed layout present but unusable (width mismatch): the build falls
        // back to rows and the dropped layout still counts as wasted.
        let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
        let mut chunk = chunk_from_columns(vec![vec![Value::Int(1)], vec![Value::string("a")]])
            .with_columnar_stats(Arc::clone(&stats));
        chunk.build_typed_columns(true);
        chunk.typed_columns.as_mut().expect("typed layout").pop();
        let mut side = HashJoinBuildSide::new();
        side.insert_chunk(&mut chunk, &[], &[])
            .expect("fallback insert");
        assert_eq!(side.row_count(), 1);
        assert_eq!(stats.columnar_wasted_builds.load(Ordering::Relaxed), 1);

        // No typed layout at all: nothing to waste.
        let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
        let mut chunk =
            chunk_from_columns(vec![vec![Value::Int(1)]]).with_columnar_stats(Arc::clone(&stats));
        let mut side = HashJoinBuildSide::new();
        side.insert_chunk(&mut chunk, &[], &[])
            .expect("plain insert");
        assert_eq!(stats.columnar_wasted_builds.load(Ordering::Relaxed), 0);
    }
